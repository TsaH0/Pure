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

pub mod install;
pub use helix_plugin::PluginCompletion;
pub use install::{install_command, remove_command, update_command};
mod debug;
mod http;
mod listen;
mod lsp_server;
mod mouse;
mod picker;
mod popup;
mod process;
mod prompt;
mod watch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
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
    static PENDING: Mutex<Pending> = Mutex::new(Pending { events: Vec::new(), events2: Vec::new(), scheduled: false });
    /// Set while plugin actions are applied, so a plugin's own edits don't
    /// produce change events (which could loop forever).
    static APPLYING: AtomicUsize = AtomicUsize::new(0);
    /// Plugin commands running inside each other (a plugin command running
    /// `:plugin-run`), to stop runaway recursion.
    static DEPTH: AtomicUsize = AtomicUsize::new(0);
    /// The events some plugin subscribed to (bits of `event_bit`), so editor
    /// hooks cost nothing when no plugin listens.
    static WANTED: AtomicU32 = AtomicU32::new(0);
    /// The same for API 0.5's events and hooks (bits of `event2_bit`).
    static WANTED2: AtomicU32 = AtomicU32::new(0);
    /// Plugin buffers being opened (their owner is told at once, but not
    /// again from inside that answer).
    static OPENING: AtomicUsize = AtomicUsize::new(0);
    /// The focused view and its document, as last reported to plugins.
    static LAST_FOCUS: Mutex<Option<(ViewId, DocumentId)>> = Mutex::new(None);
    /// The host's marks, moved by every edit (without the host's lock).
    static MARKS: Mutex<Option<helix_plugin::v05::SharedMarks>> = Mutex::new(None);
    /// What the UI shows (prompt, picker, popup) and the keys typed so far,
    /// for `state.ui`.
    static UI: Mutex<UiFlags> = Mutex::new(UiFlags { prompt: false, picker: false, popup: false, pending: Vec::new() });
    /// The workspace of the working directory, cached: `(cwd, workspace)`.
    static WORKSPACE: Mutex<Option<(PathBuf, PathBuf)>> = Mutex::new(None);
    /// Whether a plugin may read registers, so snapshots include them.
    static WANTS_REGISTERS: AtomicBool = AtomicBool::new(false);
    /// Whether an API 0.5 plugin is loaded (it can read the completion
    /// menu's items and `state.ui`).
    static HAS_V05: AtomicBool = AtomicBool::new(false);
    /// Completion requests waiting for a plugin's answer: `(plugin, id)`.
    static COMPLETIONS: Mutex<Vec<(String, u32, tokio::sync::oneshot::Sender<Vec<PluginCompletion>>)>> = Mutex::new(Vec::new());
    /// A background load is running: directories wait for it.
    static LOADING: AtomicBool = AtomicBool::new(false);
    /// The directory to hand to `editor.directory-handler` once the
    /// background load finishes (the latest request wins).
    static PENDING_DIRECTORY: Mutex<Option<PathBuf>> = Mutex::new(None);
    /// Counts loads, so a slow background load can't replace a newer one.
    static GENERATION: AtomicUsize = AtomicUsize::new(0);
    /// The plugin directory and settings loaded last, to reload them.
    static LOADED: Mutex<Option<(PathBuf, toml::Table)>> = Mutex::new(None);
    /// Running timers: `(plugin, id, task)`, aborted when cancelled or when
    /// their plugin goes.
    static TIMERS: Mutex<Vec<(String, u32, tokio::task::AbortHandle)>> = Mutex::new(Vec::new());
}

/// How many plugin timers are running (for tests).
#[doc(hidden)]
pub fn running_timers() -> usize {
    TIMERS.lock().len()
}

/// Stop the timers of `plugin` (`None`: all of them).
fn abort_timers(plugin: Option<&str>) {
    TIMERS.lock().retain(|(owner, _, task)| {
        let keep = plugin.is_some_and(|plugin| plugin != owner);
        if !keep {
            task.abort();
        }
        keep
    });
}

/// Remove what a disabled plugin left behind: processes, timers, completion
/// requests, decorations, diagnostics, statusline text and its sidebar.
fn tear_down(editor: &mut Editor, plugin: &str) {
    plugin_changed(plugin, helix_plugin::v05::PluginState::Disabled);
    process::kill_plugin(plugin);
    abort_timers(Some(plugin));
    listen::stop_plugin(Some(plugin));
    watch::stop_plugin(Some(plugin));
    http::stop_plugin(Some(plugin));
    lsp_server::stop_plugin(editor, Some(plugin));
    mouse::stop_plugin(Some(plugin));
    debug::stop_plugin(Some(plugin));
    crate::keymap::modes::remove_plugin(Some(plugin));
    editor.plugin_bars.clear_plugin(Some(plugin));
    clear_key_bindings(Some(plugin));
    COMPLETIONS.lock().retain(|(owner, _, _)| owner != plugin);
    for doc in editor.documents.values_mut() {
        if !doc.plugin_decorations.is_empty() {
            doc.plugin_decorations.clear_plugin(plugin);
        }
    }
    editor.clear_plugin_diagnostics(plugin);
    let prefix = format!("{plugin}/");
    editor
        .plugin_statusline
        .retain(|key, _| !key.starts_with(&prefix));
    editor.panels.retain(|panel| panel.owner != plugin);
    helix_event::request_redraw();
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
        EventKind::Completion => 10,
        EventKind::BufferWrite => 11,
    }
}

const EVENT_KINDS: [EventKind; 11] = [
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
    EventKind::Completion,
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
    /// API 0.5 events, with the document they are about.
    events2: Vec<(helix_plugin::v05::Event2, Option<u32>)>,
    scheduled: bool,
}

/// The API 0.5 events and hooks, as bits of [`WANTED2`].
const EVENT2_KINDS: [helix_plugin::v05::SubscriptionEvent; 10] = {
    use helix_plugin::v05::SubscriptionEvent as S;
    [
        S::BufferEnter,
        S::BufferLeave,
        S::ViewEnter,
        S::ViewLeave,
        S::CompletionMenu,
        S::ConfigChange,
        S::PluginChange,
        S::Shutdown,
        S::PreCommand,
        S::PreSave,
    ]
};

fn event2_bit(kind: helix_plugin::v05::SubscriptionEvent) -> u32 {
    EVENT2_KINDS
        .iter()
        .position(|k| *k == kind)
        .map_or(0, |i| 1 << i)
}

/// Whether some plugin subscribed to `kind` (one atomic load).
fn wants2(kind: helix_plugin::v05::SubscriptionEvent) -> bool {
    WANTED2.load(Ordering::Relaxed) & event2_bit(kind) != 0
}

fn wanted2_mask(host: &PluginHost) -> u32 {
    EVENT2_KINDS
        .into_iter()
        .filter(|kind| host.wants2(*kind))
        .map(event2_bit)
        .fold(0, |mask, bit| mask | bit)
}

/// Queue an API 0.5 event about `doc` for the plugins that subscribed.
fn queue2(event: helix_plugin::v05::Event2, doc: Option<u32>) {
    if !wants2(helix_plugin::v05::SubscriptionEvent::of(&event)) {
        return;
    }
    let mut pending = PENDING.lock();
    pending.events2.push((event, doc));
    if !pending.scheduled {
        pending.scheduled = true;
        tokio::spawn(async {
            tokio::time::sleep(EVENT_DELAY).await;
            job::dispatch(|editor, _| deliver_events(editor)).await;
        });
    }
}

/// The completion menu opened, changed or closed: tell the plugins that
/// subscribed.
pub fn completion_menu_event(
    editor: &Editor,
    event: crate::ui::completion::menu_events::MenuEvent,
) {
    use crate::ui::completion::menu_events::{Close, MenuEvent};
    use helix_plugin::v05::{CloseReason, CompletionEvent, Event2, SubscriptionEvent};
    if !wants2(SubscriptionEvent::CompletionMenu) {
        return;
    }
    let event = match event {
        MenuEvent::Opened(n) => CompletionEvent::Opened(n),
        MenuEvent::Updated(n) => CompletionEvent::Updated(n),
        MenuEvent::Confirmed(label) => CompletionEvent::Confirmed(label),
        MenuEvent::Closed(reason) => CompletionEvent::Closed(match reason {
            Close::Aborted => CloseReason::Aborted,
            Close::Confirmed => CloseReason::Confirmed,
            Close::Dismissed => CloseReason::Dismissed,
            Close::ModeChange => CloseReason::ModeChange,
            Close::Replaced => CloseReason::Replaced,
        }),
    };
    let doc = plugin_doc_id(editor.tree.get(editor.tree.focus).doc);
    queue2(Event2::CompletionMenu(event), Some(doc));
}

/// Apply what the plugins a hook asked did; `Err(plugin: reason)` if one
/// cancelled.
fn finish_hook(
    editor: &mut Editor,
    jobs: &mut Jobs,
    result: helix_plugin::v05::HookResult,
) -> Result<(), String> {
    for (plugin, actions) in result.results {
        report_with(editor, Some(&mut *jobs), &plugin, actions);
    }
    match result.cancelled {
        Some((plugin, reason)) => Err(format!("{plugin}: {reason}")),
        None => Ok(()),
    }
}

/// Before `command` runs (a native command's or a typable command's name):
/// the plugins hooked to it may cancel it (`Err`). Commands plugins run
/// themselves aren't hooked.
pub fn pre_command(editor: &mut Editor, jobs: &mut Jobs, command: &str) -> Result<(), String> {
    pre_command_with(editor, jobs, command, None, None)
}

runtime_local! {
    /// The register and count of the command `pre-command` hooks are asked
    /// about (`state.command-register` and `command-count`).
    static COMMAND_ARGS: parking_lot::Mutex<(Option<char>, Option<u32>)> =
        parking_lot::Mutex::new((None, None));
}

/// [`pre_command`] for a command given `register` and `count`.
pub fn pre_command_with(
    editor: &mut Editor,
    jobs: &mut Jobs,
    command: &str,
    register: Option<char>,
    count: Option<std::num::NonZeroUsize>,
) -> Result<(), String> {
    use helix_plugin::v05::SubscriptionEvent;
    if !wants2(SubscriptionEvent::PreCommand)
        || APPLYING.load(Ordering::Relaxed) > 0
        || !with_host(|host| host.hooks_command(command)).unwrap_or(false)
    {
        return Ok(());
    }
    *COMMAND_ARGS.lock() = (register, count.map(|count| count.get() as u32));
    let snapshot = snapshot(editor);
    *COMMAND_ARGS.lock() = (None, None);
    let Some(result) = with_host(|host| host.pre_command(command, &snapshot)) else {
        return Ok(());
    };
    finish_hook(editor, jobs, result)
}

/// Before `doc` is written: the plugins hooked to saving may edit it (saved
/// with it) or cancel (`Err`).
pub fn pre_save(editor: &mut Editor, jobs: &mut Jobs, doc: DocumentId) -> Result<(), String> {
    use helix_plugin::v05::SubscriptionEvent;
    if !wants2(SubscriptionEvent::PreSave) || APPLYING.load(Ordering::Relaxed) > 0 {
        return Ok(());
    }
    let snapshot = snapshot(editor);
    let Some(result) = with_host(|host| host.pre_save(plugin_doc_id(doc), &snapshot)) else {
        return Ok(());
    };
    finish_hook(editor, jobs, result)
}

fn view_number(view: ViewId) -> u64 {
    use slotmap::Key;
    view.data().as_ffi()
}

/// Called each frame: tell plugins that subscribed when the focused view,
/// or the document it shows, changed.
pub fn check_focus(editor: &Editor) {
    use helix_plugin::v05::{Event2, SubscriptionEvent as S};
    let wanted = [S::BufferEnter, S::BufferLeave, S::ViewEnter, S::ViewLeave];
    if !wanted.into_iter().any(wants2) {
        return;
    }
    let view = editor.tree.focus;
    let Some(doc) = editor.tree.try_get(view).map(|view| view.doc) else {
        return;
    };
    let previous = LAST_FOCUS.lock().replace((view, doc));
    if previous == Some((view, doc)) {
        return;
    }
    if let Some((old_view, old_doc)) = previous {
        if editor.documents.contains_key(&old_doc) {
            queue2(
                Event2::BufferLeave(plugin_doc_id(old_doc), view_number(old_view)),
                Some(plugin_doc_id(old_doc)),
            );
        }
        if old_view != view {
            queue2(Event2::ViewLeave(view_number(old_view)), None);
        }
    }
    if previous.is_none_or(|(old_view, _)| old_view != view) {
        queue2(Event2::ViewEnter(view_number(view)), None);
    }
    queue2(
        Event2::BufferEnter(plugin_doc_id(doc), view_number(view)),
        Some(plugin_doc_id(doc)),
    );
}

/// The editor is quitting: tell the plugins that subscribed, now.
pub fn shutdown(editor: &mut Editor) {
    use helix_plugin::v05::{Event2, SubscriptionEvent};
    if !wants2(SubscriptionEvent::Shutdown) {
        return;
    }
    let snapshot = snapshot(editor);
    let Some(results) = with_host(|host| host.dispatch2(&Event2::Shutdown, None, &snapshot)) else {
        return;
    };
    for (plugin, result) in results {
        report(editor, &plugin, result);
    }
}

/// Tell the plugins that subscribed that `plugin` changed state.
fn plugin_changed(plugin: &str, state: helix_plugin::v05::PluginState) {
    queue2(
        helix_plugin::v05::Event2::PluginChange(plugin.to_string(), state),
        None,
    );
}

/// Run a plugin's completion menu command on the editor view.
fn menu_command(
    editor: &mut Editor,
    compositor: &mut Compositor,
    command: helix_plugin::v05::MenuAction,
) {
    use crate::ui::completion::menu_events::MenuCommand as C;
    use helix_plugin::v05::MenuAction as A;
    let command = match command {
        A::Select(index) => C::Select(index),
        A::Next => C::Next,
        A::Previous => C::Previous,
        A::Confirm => C::Confirm,
        A::Abort => C::Abort,
        A::Close => C::Close,
    };
    let Some(view) = compositor.find::<crate::ui::EditorView>() else {
        return;
    };
    let mut jobs = Jobs::new();
    let mut cx = crate::compositor::Context {
        editor,
        jobs: &mut jobs,
        scroll: None,
    };
    view.completion_command(command, &mut cx);
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

/// `plugins.lock`, next to the plugins directory `dir`.
pub fn lock_file(dir: &Path) -> PathBuf {
    dir.with_file_name("plugins.lock")
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
    if !has_plugins(&dir, &toml::Table::new()) {
        return;
    }
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    LOADING.store(true, Ordering::Relaxed);
    tokio::spawn(async move {
        let (settings, error) = read_settings();
        let built = tokio::task::spawn_blocking(move || {
            let (host, actions, mut errors) = build_host(&dir, &settings);
            errors.extend(error);
            (host, actions, errors, (dir, settings))
        })
        .await;
        let Ok((host, actions, errors, loaded)) = built else {
            LOADING.store(false, Ordering::Relaxed);
            let pending = PENDING_DIRECTORY.lock().take();
            if let Some(dir) = pending {
                job::dispatch(move |editor, compositor| {
                    open_file_picker(editor, compositor, dir, "loading plugins failed")
                })
                .await;
            }
            return;
        };
        job::dispatch(move |editor, _| {
            install_background(editor, generation, host, actions, errors, loaded);
        })
        .await;
    });
}

/// Pretend a background load started (for tests).
#[doc(hidden)]
pub fn start_loading_for_tests() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
    LOADING.store(true, Ordering::Relaxed);
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
    finish_loading(editor);
}

/// Plugins are loaded: open the directory that waited for them.
fn finish_loading(editor: &Editor) {
    LOADING.store(false, Ordering::Relaxed);
    if let Some(dir) = PENDING_DIRECTORY.lock().take() {
        if let Some(handler) = editor.config().directory_handler.clone() {
            run_directory_handler(handler, dir);
        }
    }
}

/// Open `dir` with `editor.directory-handler`, if one is set: now, or once
/// plugins finished loading. `false` means no handler: the caller opens its
/// file picker. If the handler fails, the file picker opens instead.
pub fn open_directory(editor: &Editor, dir: PathBuf) -> bool {
    let Some(handler) = editor.config().directory_handler.clone() else {
        return false;
    };
    if LOADING.load(Ordering::Relaxed) {
        *PENDING_DIRECTORY.lock() = Some(dir);
    } else {
        run_directory_handler(handler, dir);
    }
    true
}

/// Run `handler` with `dir` once the current event is handled.
fn run_directory_handler(handler: String, dir: PathBuf) {
    tokio::spawn(job::dispatch_callback(job::Callback::EditorCompositorJobs(
        Box::new(move |editor, compositor, jobs| {
            let arg = dir.to_string_lossy().into_owned();
            if let Err(err) = run_command(editor, jobs, &handler, vec![arg]) {
                open_file_picker(editor, compositor, dir, &format!("{handler}: {err}"));
            }
        }),
    )));
}

/// The fallback when the directory handler can't open `dir`.
fn open_file_picker(editor: &mut Editor, compositor: &mut Compositor, dir: PathBuf, why: &str) {
    editor.set_error(format!(
        "directory-handler failed ({why}); showing the file picker"
    ));
    let picker = crate::ui::file_picker(editor, dir);
    compositor.push(Box::new(crate::ui::overlay::overlaid(picker)));
}

/// Whether `dir` has a plugin that isn't turned off in `settings` (an
/// unreadable manifest counts, so its error is shown).
fn has_plugins(dir: &Path, settings: &toml::Table) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let manifest = entry.path().join("plugin.toml");
            manifest.is_file()
                && std::fs::read_to_string(&manifest)
                    .ok()
                    .and_then(|source| helix_plugin::Manifest::parse(&source).ok())
                    .is_none_or(|manifest| plugin_settings(settings, &manifest.name).is_some())
        })
    })
}

/// Start a plugin host with the plugins in `dir`; `None` if there are none.
fn build_host(
    dir: &Path,
    settings: &toml::Table,
) -> (Option<PluginHost>, Vec<Action>, Vec<anyhow::Error>) {
    if !has_plugins(dir, settings) {
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
    host.set_program_check(starts_program);
    host.set_editor_fns(editor_fns());
    let mut setting_errors = Vec::new();
    // Installed plugins whose component changed since aren't loaded.
    // An unreadable lock can't tell which plugins were installed: load none.
    let (changed, lock_broken) = match install::Lock::read(&lock_file(dir)) {
        Ok(lock) => (install::verify(dir, &lock), false),
        Err(err) => {
            setting_errors.push(anyhow::anyhow!(
                "{err:#}: no plugins were loaded (fix or delete plugins.lock, then \
                 :plugin-reload)"
            ));
            (Vec::new(), true)
        }
    };
    let (actions, mut errors) = host.load_all(dir, |name| {
        if lock_broken {
            return None;
        }
        if let Some((_, reason)) = changed.iter().find(|(changed, _)| changed == name) {
            setting_errors.push(anyhow::anyhow!("{reason}"));
            return None;
        }
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
    mut errors: Vec<anyhow::Error>,
) {
    let settings = LOADED.lock().as_ref().map(|(_, settings)| settings.clone());
    let keys = host.as_ref().map_or(Ok((None, None)), |host| {
        plugin_keys(
            &host.info(),
            settings.as_ref().unwrap_or(&toml::Table::new()),
        )
    });
    let (keys, buffer_keys) = keys.unwrap_or_else(|err| {
        errors.push(err);
        (None, None)
    });
    crate::keymap::set_plugin_keys(keys);
    crate::keymap::set_buffer_keys(buffer_keys);
    // The previous plugins' decorations go with them.
    for doc in editor.documents.values_mut() {
        if !doc.plugin_decorations.is_empty() {
            doc.plugin_decorations = Default::default();
        }
    }
    editor.plugin_statusline.clear();
    editor.panels.clear();
    abort_timers(None);
    listen::stop_plugin(None);
    watch::stop_plugin(None);
    http::stop_plugin(None);
    lsp_server::stop_plugin(editor, None);
    mouse::stop_plugin(None);
    debug::stop_plugin(None);
    crate::keymap::modes::remove_plugin(None);
    editor.plugin_bars.clear_plugin(None);
    clear_key_bindings(None);
    let wanted = host.as_ref().map_or(0, |host| {
        EVENT_KINDS
            .into_iter()
            .filter(|kind| host.wants(*kind))
            .map(event_bit)
            .fold(0, |mask, bit| mask | bit)
    });
    WANTED.store(wanted, Ordering::Relaxed);
    WANTED2.store(host.as_ref().map_or(0, wanted2_mask), Ordering::Relaxed);
    if let Some(host) = &host {
        for plugin in host.plugins() {
            if plugin.disabled().is_none() {
                plugin_changed(plugin.name(), helix_plugin::v05::PluginState::Loaded);
            }
        }
    }
    WANTS_REGISTERS.store(
        host.as_ref().is_some_and(|host| host.wants_registers()),
        Ordering::Relaxed,
    );
    HAS_V05.store(host.as_ref().is_some_and(has_v05), Ordering::Relaxed);
    debug::WANTED.store(host.as_ref().is_some_and(wants_debug), Ordering::Relaxed);
    clear_plugin_diagnostics(editor);
    COMPLETIONS.lock().clear();
    *MARKS.lock() = host.as_ref().map(PluginHost::marks);
    *HOST.lock() = host;

    apply(editor, None, actions);
    // Lazy plugins the open documents need.
    let open: Vec<_> = editor.documents().map(|doc| doc.id()).collect();
    for doc in open {
        load_lazy_for(editor, doc);
    }
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
    *MARKS.lock() = None;
    *LOADED.lock() = Some((dir.to_path_buf(), settings.clone()));
    let (host, actions, errors) = build_host(dir, settings);
    install(editor, host, actions, errors);
    // This replaces a background load still running: it won't finish it.
    if LOADING.load(Ordering::Relaxed) {
        finish_loading(editor);
    }
}

/// Like [`load_from`] with the settings given as TOML source.
pub fn load_with_settings(editor: &mut Editor, dir: &Path, settings: &str) -> anyhow::Result<()> {
    let settings: toml::Table = toml::from_str(settings)?;
    load_from(editor, dir, &settings);
    Ok(())
}

/// `:config-reload`: read `plugins.toml` again and apply what changed.
pub fn config_reloaded(editor: &mut Editor) {
    let (settings, error) = read_settings();
    match error {
        Some(err) => editor.set_error(err.to_string()),
        None => settings_changed(editor, &settings),
    }
}

/// The user's plugin settings are now `settings`: plugins whose own
/// settings changed get a `config-change` event; if what a plugin is
/// granted (or which plugins are enabled) changed, they are all reloaded.
pub fn settings_changed(editor: &mut Editor, settings: &toml::Table) {
    let Some((dir, old)) = LOADED.lock().clone() else {
        return;
    };
    if &old == settings {
        return;
    }
    // Grants, keys and `enabled` decide what is loaded and allowed.
    let fixed = |table: &toml::Table| -> std::collections::BTreeMap<String, toml::Value> {
        table
            .iter()
            .map(|(name, value)| {
                let value = match value {
                    toml::Value::Table(table) => toml::Value::Table(
                        table
                            .iter()
                            .filter(|(key, _)| {
                                key.starts_with("allow-") || *key == "enabled" || *key == "keys"
                            })
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect(),
                    ),
                    other => other.clone(),
                };
                (name.clone(), value)
            })
            .filter(|(_, value)| value.as_table().is_none_or(|table| !table.is_empty()))
            .collect()
    };
    if fixed(&old) != fixed(settings) {
        load_from(editor, &dir, settings);
        editor.set_status("Plugins reloaded: their permissions or keys changed");
        return;
    }
    *LOADED.lock() = Some((dir, settings.clone()));
    let names: Vec<String> = with_host(|host| {
        host.plugins()
            .iter()
            .map(|plugin| plugin.name().to_string())
            .collect()
    })
    .unwrap_or_default();
    for name in names {
        let Some(Ok(plugin_settings)) = plugin_settings(settings, &name) else {
            continue;
        };
        let snapshot = snapshot(editor);
        match with_host(|host| host.set_config(&name, &plugin_settings.config, &snapshot)) {
            Some(Ok(Some(actions))) => apply(editor, None, actions),
            Some(Err(err)) => editor.set_error(format!("{err:#}")),
            _ => {}
        }
    }
    refresh_wanted(editor);
}

/// The report of `:plugin-doctor`.
pub fn doctor() -> String {
    with_host(|host| host.doctor()).unwrap_or_else(|| "No plugins are loaded.".into())
}

/// Trace `plugin`'s calls (or all plugins'), or stop: how many plugins.
pub fn set_trace(plugin: Option<&str>, on: bool) -> usize {
    with_host(|host| host.set_trace(plugin, on)).unwrap_or(0)
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
    table.remove("allow-dirs");
    table.remove("allow-listen");
    table.remove("allow-key-override");
    table.remove("allow-lsp");
    table.remove("allow-debug");
    table.remove("allow-network");
    table.remove("lazy");
    table.remove("keys");
    let config = serde_json::to_string(&table).unwrap_or_else(|_| "{}".into());
    Some((|| {
        Ok(PluginSettings {
            config,
            process: process_grant(settings, name)?,
            files: file_grant(settings, name)?,
            dirs: allowed_dirs(settings, name)?,
            listen: allowed_ports(settings, name)?,
            key_override: flag(settings, name, "allow-key-override")?,
            lsp: flag(settings, name, "allow-lsp")?,
            debug: flag(settings, name, "allow-debug")?,
            network: allowed_hosts(settings, name)?,
            eager: settings
                .get(name)
                .and_then(|table| table.get("lazy"))
                .is_some_and(|lazy| lazy.as_bool() == Some(false)),
        })
    })())
}

/// A `true`/`false` setting of plugin `name` (false if missing).
fn flag(settings: &toml::Table, name: &str, key: &str) -> anyhow::Result<bool> {
    match settings.get(name).and_then(|table| table.get(key)) {
        None => Ok(false),
        Some(toml::Value::Boolean(allowed)) => Ok(*allowed),
        Some(_) => anyhow::bail!("[{name}] {key}: expected true or false"),
    }
}

type Keys = HashMap<Mode, crate::keymap::KeyTrie>;
/// Default bindings, and bindings for each plugin's own buffers.
type PluginKeys = (Option<Keys>, Option<HashMap<String, Keys>>);

/// The plugins' `[keys]`, as one keymap: each value is one of the plugin's
/// commands, bound to `:plugin-run <plugin>/<command>`. `keys = false` in a
/// plugin's `plugins.toml` table leaves its keys out. Earlier plugins (by
/// directory name) win over later ones; the user's and Pure's keys over all.
/// Also the bindings for the plugins' own buffers (`[keys.buffer]`), by
/// plugin.
fn plugin_keys(
    plugins: &[helix_plugin::PluginInfo],
    settings: &toml::Table,
) -> anyhow::Result<PluginKeys> {
    fn translate(
        plugin: &helix_plugin::PluginInfo,
        value: &toml::Value,
    ) -> anyhow::Result<toml::Value> {
        match value {
            toml::Value::String(command) => {
                anyhow::ensure!(
                    plugin.commands.iter().any(|c| &c.name == command),
                    "plugin '{}': [keys] names '{command}', which is not one of its commands",
                    plugin.name
                );
                Ok(toml::Value::String(format!(
                    ":plugin-run {}/{command}",
                    plugin.name
                )))
            }
            toml::Value::Table(table) => Ok(toml::Value::Table(
                table
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), translate(plugin, value)?)))
                    .collect::<anyhow::Result<_>>()?,
            )),
            _ => anyhow::bail!(
                "plugin '{}': [keys] values must be command names or tables",
                plugin.name
            ),
        }
    }
    let mut all: Option<HashMap<Mode, crate::keymap::KeyTrie>> = None;
    let mut buffers: HashMap<String, Keys> = HashMap::new();
    for plugin in plugins.iter().filter(|plugin| plugin.disabled.is_none()) {
        let Some(keys) = &plugin.keys else {
            continue;
        };
        let mut keys = keys.clone();
        // Buffer keys can't be turned off: they only apply in the plugin's
        // own buffers.
        if let Some(buffer) = keys.remove("buffer") {
            let translated = translate(plugin, &buffer)?;
            let keymap: Keys = translated.try_into().map_err(|err| {
                anyhow::anyhow!("plugin '{}': invalid [keys.buffer]: {err}", plugin.name)
            })?;
            buffers.insert(plugin.name.clone(), keymap);
        }
        let wanted = settings
            .get(&plugin.name)
            .and_then(|table| table.get("keys"))
            .and_then(toml::Value::as_bool)
            .unwrap_or(true);
        if !wanted {
            continue;
        }
        if keys.is_empty() {
            continue;
        }
        let translated = translate(plugin, &toml::Value::Table(keys))?;
        let keymap: HashMap<Mode, crate::keymap::KeyTrie> = translated
            .try_into()
            .map_err(|err| anyhow::anyhow!("plugin '{}': invalid [keys]: {err}", plugin.name))?;
        match &mut all {
            None => all = Some(keymap),
            Some(all) => {
                for (mode, trie) in keymap {
                    match (all.get_mut(&mode), trie) {
                        (
                            Some(crate::keymap::KeyTrie::Node(node)),
                            crate::keymap::KeyTrie::Node(extra),
                        ) => node.fill(extra),
                        (None, trie) => {
                            all.insert(mode, trie);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok((all, (!buffers.is_empty()).then_some(buffers)))
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

/// `allow-dirs` for plugin `name`: directories besides the workspace that
/// `allow-files` applies to (`~` is the home directory).
fn allowed_dirs(settings: &toml::Table, name: &str) -> anyhow::Result<Vec<PathBuf>> {
    let Some(value) = settings.get(name).and_then(|table| table.get("allow-dirs")) else {
        return Ok(Vec::new());
    };
    let error = || anyhow::anyhow!("[{name}] allow-dirs: expected a list of absolute directories");
    value
        .as_array()
        .ok_or_else(error)?
        .iter()
        .map(|dir| {
            let dir = helix_stdx::path::expand_tilde(Path::new(dir.as_str().ok_or_else(error)?));
            anyhow::ensure!(dir.is_absolute(), error());
            Ok(helix_stdx::path::canonicalize(dir))
        })
        .collect()
}

/// `allow-listen` for plugin `name`: the ports it may listen on.
fn allowed_ports(settings: &toml::Table, name: &str) -> anyhow::Result<Vec<u16>> {
    let Some(value) = settings
        .get(name)
        .and_then(|table| table.get("allow-listen"))
    else {
        return Ok(Vec::new());
    };
    let error = || anyhow::anyhow!("[{name}] allow-listen: expected a list of ports (1-65535)");
    value
        .as_array()
        .ok_or_else(error)?
        .iter()
        .map(|port| {
            port.as_integer()
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port != 0)
                .ok_or_else(error)
        })
        .collect()
}

/// `allow-network` for plugin `name`: the hosts it may send HTTPS requests to.
fn allowed_hosts(settings: &toml::Table, name: &str) -> anyhow::Result<Vec<String>> {
    let Some(value) = settings
        .get(name)
        .and_then(|table| table.get("allow-network"))
    else {
        return Ok(Vec::new());
    };
    let error = || {
        anyhow::anyhow!(
            "[{name}] allow-network: expected a list of hosts (\"api.example.com\", \"*.example.org\")"
        )
    };
    value
        .as_array()
        .ok_or_else(error)?
        .iter()
        .map(|host| {
            host.as_str()
                .filter(|host| helix_plugin::http::valid_pattern(host))
                .map(str::to_owned)
                .ok_or_else(error)
        })
        .collect()
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

/// `(plugin/command, doc)` of all plugin commands, for the command palette.
pub fn commands_with_docs() -> Vec<(String, String)> {
    with_host(|host| {
        host.commands()
            .into_iter()
            .map(|(plugin, command)| (format!("{plugin}/{}", command.name), command.doc))
            .collect()
    })
    .unwrap_or_default()
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
struct UiFlags {
    prompt: bool,
    picker: bool,
    popup: bool,
    pending: Vec<helix_view::input::KeyEvent>,
}

fn wants_debug(host: &PluginHost) -> bool {
    host.plugins().iter().any(|p| {
        p.disabled().is_none()
            && p.manifest()
                .permissions
                .contains(&helix_plugin::Permission::Debug)
    })
}

fn has_v05(host: &PluginHost) -> bool {
    host.plugins()
        .iter()
        .any(|p| p.disabled().is_none() && p.manifest().at_least_0_5())
}

/// Whether a plugin can read the completion menu's items.
pub fn reads_menu() -> bool {
    HAS_V05.load(Ordering::Relaxed)
}

/// The compositor's layers, each frame.
pub fn set_ui_layers(layers: impl Iterator<Item = &'static str>) {
    if !HAS_V05.load(Ordering::Relaxed) {
        return;
    }
    let (mut prompt, mut picker, mut popup) = (false, false, false);
    for layer in layers {
        prompt |= layer.contains("ui::prompt::Prompt");
        picker |= layer.contains("ui::picker::Picker");
        popup |= layer.contains("ui::popup::Popup");
    }
    let mut ui = UI.lock();
    (ui.prompt, ui.picker, ui.popup) = (prompt, picker, popup);
}

/// The keys typed so far of a sequence, each frame.
pub fn set_pending_keys(keys: &[helix_view::input::KeyEvent]) {
    if !HAS_V05.load(Ordering::Relaxed) {
        return;
    }
    let mut ui = UI.lock();
    if ui.pending != keys {
        ui.pending = keys.to_vec();
    }
}

/// The views and UI, for snapshots.
fn views_and_ui(editor: &Editor) -> (Vec<helix_plugin::v05::ViewInfo>, helix_plugin::v05::UiState) {
    let focus = editor.tree.focus;
    let views = editor
        .tree
        .views()
        .map(|(view, focused)| {
            let doc = &editor.documents[&view.doc];
            let text = doc.text().slice(..);
            let first = text.char_to_line(doc.view_offset(view.id).anchor.min(text.len_chars()));
            let last = (first + view.inner_height().saturating_sub(1))
                .min(text.len_lines().saturating_sub(1));
            let area = view.area;
            helix_plugin::v05::ViewInfo {
                id: view_number(view.id),
                document: plugin_doc_id(view.doc),
                area: (area.x, area.y, area.width, area.height),
                first_line: first as u32,
                last_line: last as u32,
                focused: focused || view.id == focus,
            }
        })
        .collect();
    let flags = UI.lock();
    let pending: Vec<String> = flags.pending.iter().map(|key| key.to_string()).collect();
    let (command_register, command_count) = *COMMAND_ARGS.lock();
    let ui = helix_plugin::v05::UiState {
        mode: plugin_mode(editor.mode()),
        completion_menu: crate::ui::completion::menu_events::is_open(),
        prompt: flags.prompt,
        picker: flags.picker,
        popup: flags.popup,
        focused_panel: editor
            .panels
            .iter()
            .find(|panel| panel.focused)
            .map(|panel| {
                match panel.dock {
                    helix_view::sidebar::Dock::Left => "left",
                    helix_view::sidebar::Dock::Right => "right",
                    helix_view::sidebar::Dock::Bottom => "bottom",
                    helix_view::sidebar::Dock::Float => "float",
                }
                .to_string()
            }),
        pending_keys: pending.join(" "),
        command_register,
        command_count,
    };
    (views, ui)
}

/// Every native and typable command, for `commands.list`.
fn command_entries() -> Vec<helix_plugin::v05::CommandEntry> {
    use helix_plugin::v05::{CommandEntry, CommandKind};
    let native = MappableCommand::STATIC_COMMAND_LIST
        .iter()
        .map(|command| CommandEntry {
            name: command.name().to_string(),
            doc: command.doc().to_string(),
            kind: CommandKind::Native,
        });
    let typable = commands::typed::typable_commands().map(|(name, doc)| CommandEntry {
        name: name.to_string(),
        doc: doc.to_string(),
        kind: CommandKind::Typable,
    });
    native.chain(typable).collect()
}

/// What the editor provides to API 0.5 calls.
fn editor_fns() -> helix_plugin::v05::EditorFns {
    helix_plugin::v05::EditorFns {
        check_keys: Some(|keys| parse_keys(keys).map(|_| ())),
        commands: Some(command_entries),
    }
}

/// Run command `name` for a plugin (API 0.5): a plugin's "plugin/command",
/// a typable command with `args` as they are, or a native command `count`
/// times.
fn execute_named(
    editor: &mut Editor,
    jobs: &mut Jobs,
    name: &str,
    args: Vec<String>,
    count: Option<u32>,
) -> Result<(), String> {
    if name.contains('/') {
        return run_command(editor, jobs, name, args).map_err(|err| err.to_string());
    }
    if commands::typed::typable_commands().any(|(typable, _)| typable == name) {
        let mut cx = crate::compositor::Context {
            editor,
            jobs,
            scroll: None,
        };
        return commands::typed::execute_with_args(&mut cx, name, &args)
            .map_err(|err| err.to_string());
    }
    let command = MappableCommand::from_str(name).map_err(|err| err.to_string())?;
    run_static(editor, jobs, &command, count);
    Ok(())
}

/// Run a native command as its key would, reporting a mode change.
fn run_static(editor: &mut Editor, jobs: &mut Jobs, command: &MappableCommand, count: Option<u32>) {
    let old_mode = editor.mode();
    let mut cx = commands::Context {
        register: None,
        count: count.and_then(|count| std::num::NonZeroUsize::new(count as usize)),
        editor,
        callback: Vec::new(),
        on_next_key_callback: None,
        jobs,
    };
    command.execute(&mut cx);
    let new_mode = cx.editor.mode();
    if new_mode != old_mode {
        helix_event::dispatch(crate::events::OnModeSwitch {
            old_mode,
            new_mode,
            cx: &mut cx,
        });
    }
    for callback in cx.callback {
        crate::compositor::defer_ui(crate::compositor::DeferredUi::Callback(callback));
    }
    if let Some((callback, kind)) = cx.on_next_key_callback {
        crate::compositor::defer_ui(crate::compositor::DeferredUi::NextKey(callback, kind));
    }
}

/// Switch mode like the mode keys.
fn set_mode(editor: &mut Editor, jobs: &mut Jobs, mode: helix_plugin::Mode) {
    let (target, command) = match mode {
        helix_plugin::Mode::Normal => (Mode::Normal, MappableCommand::normal_mode),
        helix_plugin::Mode::Insert => (Mode::Insert, MappableCommand::insert_mode),
        helix_plugin::Mode::Select => (Mode::Select, MappableCommand::select_mode),
    };
    if editor.mode() != target {
        run_static(editor, jobs, &command, None);
    }
}

/// Send a plugin's request to a language server; deliver the reply later.
fn lsp_request(
    editor: &Editor,
    plugin: String,
    id: u32,
    doc: helix_plugin::DocumentId,
    server: Option<String>,
    method: String,
    params: String,
) {
    use helix_plugin::v05::Callback2;
    let params = serde_json::from_str(&params).unwrap_or(serde_json::Value::Null);
    let reply = editor_doc_id(editor, doc).and_then(|doc| {
        let doc = editor.documents.get(&doc)?;
        doc.language_servers()
            .find(|client| server.as_deref().is_none_or(|name| client.name() == name))
            .map(|client| client.call_raw(method, params))
    });
    let Some(reply) = reply else {
        let result = Err("the language server is gone".to_string());
        return deliver2_later(plugin, Callback2::LspReply { id, result });
    };
    tokio::spawn(async move {
        let result = match reply.await {
            Ok(value) => {
                let json = value.to_string();
                if json.len() > 16 << 20 {
                    Err("the reply is larger than 16 MiB".to_string())
                } else {
                    Ok(json)
                }
            }
            Err(err) => Err(err.to_string()),
        };
        deliver2_later(plugin, Callback2::LspReply { id, result });
    });
}

/// Serve a plugin's service call after this event, then deliver the
/// reply to the caller.
fn serve_later(call: helix_plugin::v05::ServiceCall) {
    tokio::spawn(async move {
        job::dispatch_callback(job::Callback::EditorCompositorJobs(Box::new(
            move |editor, _, jobs| {
                let snapshot = snapshot(editor);
                let Some((result, actions)) = with_host(|host| host.serve(&call, &snapshot)) else {
                    return;
                };
                apply(editor, Some(&mut *jobs), actions);
                refresh_wanted(editor);
                let callback = helix_plugin::v05::Callback2::ServiceReply {
                    id: call.id,
                    result,
                };
                deliver2(editor, jobs, call.caller, callback);
            },
        )))
        .await;
    });
}

/// Deliver an API 0.5 result to `plugin` now.
fn deliver2(
    editor: &mut Editor,
    jobs: &mut Jobs,
    plugin: String,
    callback: helix_plugin::v05::Callback2,
) {
    let snapshot = snapshot(editor);
    match with_host(|host| host.callback2(&plugin, callback, &snapshot)) {
        Some(Ok(actions)) => apply(editor, Some(jobs), actions),
        Some(Err(err)) => {
            refresh_wanted(editor);
            editor.set_error(err.to_string());
        }
        None => {}
    }
}

/// Deliver an API 0.5 result to `plugin` after this event.
fn deliver2_later(plugin: String, callback: helix_plugin::v05::Callback2) {
    tokio::spawn(async move {
        job::dispatch_callback(job::Callback::EditorCompositorJobs(Box::new(
            move |editor, _, jobs| deliver2(editor, jobs, plugin, callback),
        )))
        .await;
    });
}

/// "space f" as key events.
fn parse_keys(keys: &str) -> Result<Vec<helix_view::input::KeyEvent>, String> {
    keys.split_whitespace()
        .map(|key| {
            key.parse::<helix_view::input::KeyEvent>()
                .map_err(|err| format!("invalid key '{key}': {err}"))
        })
        .collect()
}

/// Store `plugin`'s key bindings (all of them). Bindings it still has keep
/// their place in the order bindings were set; new ones go last.
fn set_key_bindings(editor: &Editor, plugin: &str, bindings: Vec<helix_plugin::v05::KeyBinding>) {
    use crate::keymap::plugin_layers::{self, Layer, PluginBinding};
    let converted: Vec<PluginBinding> = bindings
        .into_iter()
        .filter_map(|binding| {
            let keys = parse_keys(&binding.keys).ok()?;
            let document = match binding.document {
                Some(doc) => Some(editor_doc_id(editor, doc)?),
                None => None,
            };
            Some(PluginBinding {
                plugin: plugin.to_string(),
                name: binding.name,
                description: binding.description,
                layer: match binding.layer {
                    helix_plugin::v05::KeyLayer::Override => Layer::Override,
                    helix_plugin::v05::KeyLayer::Plugin => Layer::Plugin,
                    helix_plugin::v05::KeyLayer::Fallback => Layer::Fallback,
                },
                mode: match binding.mode {
                    helix_plugin::Mode::Normal => Mode::Normal,
                    helix_plugin::Mode::Insert => Mode::Insert,
                    helix_plugin::Mode::Select => Mode::Select,
                },
                keys,
                document,
                language: binding.language,
            })
        })
        .collect();
    let same = |a: &PluginBinding, b: &PluginBinding| {
        a.mode == b.mode && a.keys == b.keys && a.document == b.document
    };
    let mut all = (*plugin_layers::bindings()).clone();
    all.retain(|old| old.plugin != plugin || converted.iter().any(|new| same(old, new)));
    for new in converted {
        match all
            .iter_mut()
            .find(|old| old.plugin == plugin && same(old, &new))
        {
            Some(old) => *old = new,
            None => all.push(new),
        }
    }
    plugin_layers::set_bindings(all);
}

/// Forget `plugin`'s key bindings (`None`: everyone's).
fn clear_key_bindings(plugin: Option<&str>) {
    use crate::keymap::plugin_layers;
    let mut all = (*plugin_layers::bindings()).clone();
    all.retain(|binding| plugin.is_some_and(|plugin| binding.plugin != plugin));
    plugin_layers::set_bindings(all);
}

/// The view numbered `number` (`view_number`).
fn view_by_number(editor: &Editor, number: u64) -> Option<ViewId> {
    editor
        .tree
        .views()
        .map(|(view, _)| view.id)
        .find(|id| view_number(*id) == number)
}

/// Do what a plugin asked to splits (API 0.5 `windows`).
fn window_action(editor: &mut Editor, action: helix_plugin::v05::WindowAction) {
    use helix_plugin::v05::WindowAction as W;
    use helix_view::tree::Layout;
    let view = |editor: &Editor, number: u64| view_by_number(editor, number);
    match action {
        W::Split { doc, below } => {
            if let Some(doc) = editor_doc_id(editor, doc) {
                let action = if below {
                    helix_view::editor::Action::HorizontalSplit
                } else {
                    helix_view::editor::Action::VerticalSplit
                };
                editor.switch(doc, action);
            }
        }
        W::Focus(number) => {
            if let Some(id) = view(editor, number) {
                editor.focus(id);
            }
        }
        W::Close(number) => {
            // The last view stays: closing it would quit.
            if let Some(id) = view(editor, number).filter(|_| editor.tree.views().count() > 1) {
                editor.close(id);
            }
        }
        W::Resize {
            view: number,
            columns,
            rows,
        } => {
            if let Some(id) = view(editor, number) {
                if columns != 0 {
                    editor.tree.resize_view(id, Layout::Vertical, columns);
                }
                if rows != 0 {
                    editor.tree.resize_view(id, Layout::Horizontal, rows);
                }
            }
        }
        W::Equalize => editor.tree.equalize(),
        W::Gutters {
            view: number,
            gutters,
        } => {
            use helix_view::editor::GutterType as G;
            if let Some(id) = view(editor, number) {
                let layout = gutters
                    .iter()
                    .filter_map(|gutter| match gutter.as_str() {
                        "diagnostics" => Some(G::Diagnostics),
                        "line-numbers" => Some(G::LineNumbers),
                        "diff" => Some(G::Diff),
                        "spacer" => Some(G::Spacer),
                        "code-actions" => Some(G::CodeActionHint),
                        "plugins" => Some(G::Plugins),
                        _ => None,
                    })
                    .collect();
                editor.tree.get_mut(id).gutters.layout = layout;
            }
        }
    }
    helix_event::request_redraw();
}

/// Load the lazy plugin `name` now, if it is one (it hasn't loaded yet).
pub fn load_lazy(editor: &mut Editor, jobs: Option<&mut Jobs>, name: &str) {
    let Some(result) = with_host(|host| host.load_lazy(name)).flatten() else {
        return;
    };
    refresh_wanted(editor);
    match result {
        Ok(actions) => {
            log::info!("plugin '{name}' loaded on first use");
            plugin_changed(name, helix_plugin::v05::PluginState::Loaded);
            apply(editor, jobs, actions);
        }
        Err(err) => editor.set_error(format!("{err:#}")),
    }
}

/// Load the lazy plugins a document in `doc`'s language needs.
fn load_lazy_for(editor: &mut Editor, doc: DocumentId) {
    let Some(language) = editor
        .documents
        .get(&doc)
        .and_then(|doc| doc.language_name())
        .map(str::to_owned)
    else {
        return;
    };
    let names = with_host(|host| host.lazy_for_language(&language)).unwrap_or_default();
    for name in names {
        load_lazy(editor, None, &name);
    }
}

/// Keys bound to `plugin`'s callback `name` were pressed: ask it. `true`:
/// handled; `false`: pass the keys through. Errors (including a disabled or
/// missing plugin) are shown and pass the keys through.
pub fn key(
    editor: &mut Editor,
    jobs: &mut Jobs,
    plugin: &str,
    name: &str,
    keys: &str,
    count: Option<u32>,
) -> bool {
    use helix_plugin::v05::KeyOutcome;
    let snapshot = snapshot(editor);
    let result = with_host(|host| host.key(plugin, name, keys, count, snapshot));
    match result {
        Some(Ok((outcome, actions))) => {
            DEPTH.fetch_add(1, Ordering::Relaxed);
            apply(editor, Some(jobs), actions);
            DEPTH.fetch_sub(1, Ordering::Relaxed);
            outcome == KeyOutcome::Handled
        }
        Some(Err(err)) => {
            refresh_wanted(editor);
            editor.set_error(format!("{plugin}: {err}"));
            false
        }
        None => false,
    }
}

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
    load_lazy(editor, Some(&mut *jobs), plugin);
    let snapshot = snapshot(editor);
    let result = with_host(|host| host.run_command(plugin, command, args, snapshot))
        .ok_or_else(|| anyhow::anyhow!("no plugins are loaded"))?;
    let actions = match result {
        Ok(actions) => actions,
        Err(error) => {
            refresh_wanted(editor);
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

runtime_local! {
    /// The undo history of each document as plugins see it, kept so a
    /// snapshot only adds the revisions made since the last one.
    static HISTORIES: Mutex<std::collections::BTreeMap<DocumentId, Arc<helix_plugin::History>>> =
        Mutex::new(std::collections::BTreeMap::new());
}

runtime_local! {
    /// Each document's syntax layers as plugins see them, with the
    /// revision they are of.
    static LAYERS: Mutex<std::collections::BTreeMap<DocumentId, (i32, Arc<Vec<helix_plugin::SyntaxLayer>>)>> =
        Mutex::new(std::collections::BTreeMap::new());
}

/// `doc`'s syntax layers for plugins, made again only when the document
/// changed.
fn syntax_layers(
    editor: &Editor,
    doc: &helix_view::Document,
) -> Option<Arc<Vec<helix_plugin::SyntaxLayer>>> {
    let syntax = doc.syntax()?;
    let version = doc.version();
    let mut cache = LAYERS.lock();
    if let Some((cached, layers)) = cache.get(&doc.id()) {
        if *cached == version {
            return Some(layers.clone());
        }
    }
    let loader = editor.syn_loader.load();
    // The root first, then the others; parents by their index here.
    let mut order: Vec<_> = syntax
        .layers()
        .filter(|(_, layer)| layer.tree().is_some())
        .map(|(layer, _)| layer)
        .collect();
    order.sort_by_key(|layer| (*layer != syntax.root_layer(), layer.index()));
    let index_of = |layer: helix_core::Layer| {
        order
            .iter()
            .position(|other| *other == layer)
            .map(|i| i as u32)
    };
    let layers: Vec<_> = order
        .iter()
        .filter_map(|layer| {
            let data = syntax.layer(*layer);
            Some(helix_plugin::SyntaxLayer {
                language: loader.language(data.language).config().language_id.clone(),
                tree: data.tree()?.clone(),
                ranges: data
                    .byte_ranges()
                    .map(|range| (range.start, range.end.min(doc.text().len_bytes() as u32)))
                    .collect(),
                parent: data.parent().and_then(index_of),
            })
        })
        .collect();
    let layers = Arc::new(layers);
    cache.insert(doc.id(), (version, layers.clone()));
    Some(layers)
}

/// `doc`'s undo history for plugins: the last one, unless it changed.
fn history(doc: &helix_view::Document) -> Arc<helix_plugin::History> {
    let history = doc.history.take();
    let count = history.revision_count();
    let current = history.current_revision() as u32;
    let mut histories = HISTORIES.lock();
    let cached = histories.entry(doc.id()).or_default();
    if cached.revisions.len() != count || cached.current != current {
        let known = cached.revisions.len().min(count);
        // Revisions are only ever added: the known ones stay as they were.
        let mut revisions = cached.revisions[..known].to_vec();
        let (now, wall) = (std::time::Instant::now(), std::time::SystemTime::now());
        revisions.extend((known..count).filter_map(|n| {
            let info = history.revision_info(n)?;
            let time = wall
                .checked_sub(now.saturating_duration_since(info.timestamp))
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |time| time.as_millis() as u64);
            Some(helix_plugin::v05::Revision {
                parent: info.parent as u32,
                time,
                inserted: info.inserted as u32,
                deleted: info.deleted as u32,
            })
        }));
        *cached = Arc::new(helix_plugin::History { revisions, current });
    }
    let snapshot = cached.clone();
    drop(histories);
    doc.history.set(history);
    snapshot
}

/// A key mode `plugin` defines.
fn define_key_mode(editor: &mut Editor, plugin: String, def: helix_plugin::v05::KeyModeDef) {
    use crate::keymap::modes;
    let bindings: Vec<_> = def
        .bindings
        .iter()
        .map(|(mode, keys, command)| {
            let mode = match mode {
                helix_plugin::Mode::Normal => Mode::Normal,
                helix_plugin::Mode::Insert => Mode::Insert,
                helix_plugin::Mode::Select => Mode::Select,
            };
            (mode, keys.clone(), command.clone())
        })
        .collect();
    let keys = match modes::keys_from_bindings(&bindings) {
        Ok(keys) => keys,
        Err(err) => return editor.set_error(format!("{plugin}: mode {}: {err}", def.name)),
    };
    let mode = modes::KeyMode {
        name: def.name.clone(),
        owner: modes::Owner::Plugin(plugin.clone()),
        config: modes::ModeConfig {
            description: def.description,
            lighter: def.lighter,
            global: def.global,
            enabled: def.enabled,
            languages: def.languages,
            conflicts: def.conflicts,
            requires: def.requires,
            keys,
            on_enable: def.on_enable,
            on_disable: def.on_disable,
            chords: HashMap::new(),
        },
    };
    if let Err(err) = modes::define(mode) {
        editor.set_error(format!("{plugin}: {err}"));
    }
    helix_event::request_redraw();
}

/// Set (or remove) one of `plugin`'s bars.
fn set_bar(
    editor: &mut Editor,
    plugin: String,
    kind: helix_plugin::v05::BarKind,
    view: Option<u64>,
    bar: Option<helix_plugin::v05::Bar>,
) {
    use helix_plugin::v05::BarKind;
    use helix_view::bars;
    let spans = |spans: Vec<(String, String)>| {
        spans
            .into_iter()
            .map(|(text, scope)| bars::Span { text, scope })
            .collect()
    };
    let bar = bar.map(|bar| {
        (
            plugin.clone(),
            bars::Bar {
                left: spans(bar.left),
                center: spans(bar.center),
                right: spans(bar.right),
            },
        )
    });
    let view = match view {
        Some(number) => match view_by_number(editor, number) {
            Some(view) => Some(view),
            None => return,
        },
        None => None,
    };
    let all = &mut editor.plugin_bars;
    // Another plugin's bar stays unless this one replaces it.
    let mine = |slot: &Option<(String, bars::Bar)>| {
        slot.as_ref().is_none_or(|(owner, _)| *owner == plugin)
    };
    match (kind, view) {
        (BarKind::Statusline, None) if bar.is_some() || mine(&all.statusline) => {
            all.statusline = bar
        }
        (BarKind::Tabline, _) if bar.is_some() || mine(&all.tabline) => all.tabline = bar,
        (BarKind::Winbar, None) if bar.is_some() || mine(&all.winbar) => all.winbar = bar,
        (BarKind::Statusline, Some(view)) => set_view_bar(&mut all.statuslines, view, &plugin, bar),
        (BarKind::Winbar, Some(view)) => set_view_bar(&mut all.winbars, view, &plugin, bar),
        _ => {}
    }
    helix_event::request_redraw();
}

fn set_view_bar(
    bars: &mut HashMap<ViewId, (String, helix_view::bars::Bar)>,
    view: ViewId,
    plugin: &str,
    bar: Option<(String, helix_view::bars::Bar)>,
) {
    match bar {
        Some(bar) => {
            bars.insert(view, bar);
        }
        None => {
            if bars.get(&view).is_some_and(|(owner, _)| owner == plugin) {
                bars.remove(&view);
            }
        }
    }
}

/// A call from the editor to `plugin`'s `service`.
fn editor_call(
    plugin: &str,
    service: &str,
    method: &str,
    payload: String,
) -> helix_plugin::v05::ServiceCall {
    helix_plugin::v05::ServiceCall {
        caller: "editor".into(),
        id: 0,
        provider: plugin.into(),
        service: service.into(),
        method: method.into(),
        payload,
        chain: Vec::new(),
    }
}

/// Make `call` now and apply what the plugin asked for; its reply.
fn serve_now(
    editor: &mut Editor,
    jobs: &mut Jobs,
    call: &helix_plugin::v05::ServiceCall,
) -> Result<String, String> {
    let snapshot = snapshot(editor);
    match with_host(|host| host.serve(call, &snapshot)) {
        Some((reply, actions)) => {
            apply(editor, Some(jobs), actions);
            reply
        }
        None => Err("plugins aren't loaded".into()),
    }
}

/// Tell plugins about a mouse event; whether one took it.
pub fn mouse_event(
    editor: &mut Editor,
    jobs: &mut Jobs,
    event: &helix_view::input::MouseEvent,
) -> bool {
    mouse::handle(editor, jobs, event)
}

/// What plugins can see of the editor. Cheap: ropes are reference counted.
pub fn snapshot(editor: &Editor) -> Snapshot {
    let (views, ui) = views_and_ui(editor);
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
                    revision: doc.version().max(0) as u64,
                    language_servers: doc
                        .language_servers()
                        .map(|server| server.name().to_string())
                        .collect(),
                    folds: {
                        let text = doc.text().slice(..);
                        doc.folds
                            .iter()
                            .map(|fold| {
                                (
                                    text.char_to_line(fold.start) as u32,
                                    text.char_to_line(fold.end) as u32,
                                )
                            })
                            .collect()
                    },
                    syntax: if HAS_V05.load(Ordering::Relaxed) {
                        doc.syntax().map(|syntax| syntax.tree().clone())
                    } else {
                        None
                    },
                    key_modes: if HAS_V05.load(Ordering::Relaxed) {
                        crate::keymap::modes::active(Some(doc.id()), doc.language_name())
                            .iter()
                            .map(|mode| mode.name.clone())
                            .collect()
                    } else {
                        Vec::new()
                    },
                    history: if HAS_V05.load(Ordering::Relaxed) {
                        Some(history(doc))
                    } else {
                        None
                    },
                    layers: if HAS_V05.load(Ordering::Relaxed) {
                        syntax_layers(editor, doc)
                    } else {
                        None
                    },
                    owner: doc
                        .plugin_owner
                        .clone()
                        .zip(doc.name.as_deref().map(str::to_owned)),
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
        panels: editor
            .panels
            .iter()
            .map(|panel| helix_plugin::SidebarSnapshot {
                dock: plugin_dock(panel.dock),
                owner: panel.owner.clone(),
                selected: panel.selected as u32,
                focused: panel.focused,
            })
            .collect(),
        views,
        ui,
        completion_menu: crate::ui::completion::menu_events::snapshot().map(|menu| {
            helix_plugin::v05::MenuState {
                items: menu.items,
                selected: menu.selected,
                previewing: menu.previewing,
            }
        }),
        registers: if WANTS_REGISTERS.load(Ordering::Relaxed) {
            editor.registers.stored().collect()
        } else {
            Default::default()
        },
        debug: debug::state(editor),
        #[cfg(unix)]
        terminals: crate::terminal::plugin_terminals()
            .into_iter()
            .map(|(id, owner, read)| helix_plugin::v05::TerminalHandle { id, owner, read })
            .collect(),
        #[cfg(not(unix))]
        terminals: Vec::new(),
    }
}

/// The debugger sent an event: tell the plugins listening.
pub fn debug_event(name: &str, body: Option<&serde_json::Value>) {
    debug::event(name, body);
}

/// Apply the actions of a plugin call. `jobs` is needed to run commands; it is
/// only available when a plugin command runs (not for events).
type UiJob = Box<dyn FnOnce(&mut Editor, &mut Compositor) + Send>;

fn apply(editor: &mut Editor, mut jobs: Option<&mut Jobs>, actions: Vec<Action>) {
    APPLYING.fetch_add(1, Ordering::Relaxed);
    let mut edited: HashMap<DocumentId, ViewId> = HashMap::new();
    // UI changes need the compositor: run them later, together, in order.
    let mut ui: Vec<UiJob> = Vec::new();
    let mut actions = actions.into_iter();
    while let Some(action) = actions.next() {
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
                session,
            } => ui.push(Box::new(move |_, _| {
                if with_host(|host| {
                    host.has_request(&plugin, id, helix_plugin::PendingKind::Process)
                })
                .unwrap_or(false)
                {
                    process::spawn(plugin, id, argv, cwd, stdin, timeout, stream, session);
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
            Action::Listen { plugin, id, port } => listen::start(plugin, id, port),
            Action::SubscriptionsChanged => refresh_wanted(editor),
            Action::Execute {
                plugin,
                id,
                name,
                args,
                count,
            } => {
                let result = match jobs.as_deref_mut() {
                    Some(jobs) => execute_named(editor, jobs, &name, args, count),
                    None => Err("commands can't run from here".to_string()),
                };
                deliver2_later(
                    plugin,
                    helix_plugin::v05::Callback2::CommandDone { id, result },
                );
            }
            Action::Batch { plugin, id, steps } => {
                let mut results = Vec::with_capacity(steps.len());
                let mut failed = false;
                for step in steps {
                    if failed {
                        results.push(Err("skipped".to_string()));
                        continue;
                    }
                    let result = match (jobs.as_deref_mut(), step) {
                        (None, _) => Err("commands can't run from here".to_string()),
                        (Some(jobs), helix_plugin::v05::Step::Command { name, args, count }) => {
                            execute_named(editor, jobs, &name, args, count)
                        }
                        (Some(jobs), helix_plugin::v05::Step::SetMode(mode)) => {
                            set_mode(editor, jobs, mode);
                            Ok(())
                        }
                    };
                    failed = result.is_err();
                    results.push(result);
                }
                deliver2_later(
                    plugin,
                    helix_plugin::v05::Callback2::BatchDone { id, results },
                );
            }
            Action::ServiceCall(call) => serve_later(call),
            Action::CloseBuffer(doc) => {
                if let Some(doc_id) = editor_doc_id(editor, doc) {
                    let shown = editor.tree.views().any(|(view, _)| view.doc == doc_id);
                    let modified = editor
                        .documents
                        .get(&doc_id)
                        .is_some_and(|d| d.is_modified());
                    if !shown && !modified {
                        let _ = editor.close_document(doc_id, false);
                    }
                }
            }
            Action::FeedKeys {
                plugin,
                keys,
                may_start_programs,
            } => match helix_view::input::parse_macro(&keys) {
                // Asked by keys being fed: they would play after these,
                // and could keep feeding each other.
                Ok(_) if commands::feeding(editor) => {
                    editor.set_error(format!("{plugin}: keys being fed can't feed more keys"))
                }
                Ok(keys) => crate::compositor::defer_ui(crate::compositor::DeferredUi::Callback(
                    Box::new(move |compositor, cx| {
                        let fed = commands::feed_keys(compositor, cx, &keys, !may_start_programs);
                        if let Err(err) = fed {
                            cx.editor.set_error(format!("{plugin}: {err}"));
                        }
                    }),
                )),
                Err(err) => editor.set_error(format!("{plugin}: invalid keys: {err}")),
            },
            Action::NextKey { plugin, name } => {
                crate::compositor::defer_ui(crate::compositor::DeferredUi::NextKey(
                    Box::new(move |cx, event| {
                        key(cx.editor, cx.jobs, &plugin, &name, &event.to_string(), None);
                    }),
                    commands::OnKeyCallbackKind::PseudoPending,
                ));
            }
            Action::SetMode(mode) => {
                if let Some(jobs) = jobs.as_deref_mut() {
                    set_mode(editor, jobs, mode);
                }
            }
            // The menu is the editor view's: the rest of the actions wait
            // until it is done, to keep their order.
            Action::CompletionMenu(command) => {
                let rest: Vec<Action> = actions.by_ref().collect();
                ui.push(Box::new(move |editor, compositor| {
                    menu_command(editor, compositor, command);
                    apply(editor, None, rest);
                }));
            }
            Action::KeyBindings { plugin, bindings } => {
                set_key_bindings(editor, &plugin, bindings);
                helix_event::request_redraw();
            }
            Action::StopListen { plugin, id } => listen::stop(&plugin, id),
            Action::Watch {
                plugin,
                id,
                path,
                recursive,
            } => {
                if let Err(err) = watch::start(plugin, id, path, recursive) {
                    editor.set_error(err);
                }
            }
            Action::Unwatch { plugin, id } => watch::stop(&plugin, id),
            Action::HttpRequest {
                plugin,
                id,
                request,
            } => http::fetch(plugin, id, request),
            Action::CancelHttp { plugin, id } => http::cancel(&plugin, id),
            Action::LspRequest {
                plugin,
                id,
                doc,
                server,
                method,
                params,
            } => lsp_request(editor, plugin, id, doc, server, method, params),
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
            Action::Decorate {
                plugin,
                doc,
                layer,
                decorations,
            } => {
                let Some(doc_id) = editor_doc_id(editor, doc) else {
                    continue;
                };
                let doc = editor.documents.get_mut(&doc_id).unwrap();
                let len = doc.text().len_chars();
                let layer = format!("{plugin}/{layer}");
                let store = &mut doc.plugin_decorations;
                match decorations {
                    helix_plugin::Decorations::Highlights(highlights) => {
                        store.set_highlights(layer, highlights, len)
                    }
                    helix_plugin::Decorations::Annotations(annotations) => {
                        store.set_annotations(layer, annotations, len)
                    }
                    helix_plugin::Decorations::Overlays(overlays) => {
                        store.set_overlays(layer, overlays, len)
                    }
                    helix_plugin::Decorations::Signs(signs) => store.set_signs(
                        layer,
                        signs
                            .into_iter()
                            .map(|(pos, text, scope)| helix_view::plugin_decorations::Sign {
                                pos,
                                text,
                                scope,
                            })
                            .collect(),
                        len,
                    ),
                    helix_plugin::Decorations::LineBackgrounds(lines) => {
                        store.set_line_backgrounds(layer, lines, len)
                    }
                    helix_plugin::Decorations::Conceals(conceals) => {
                        store.set_conceals(layer, conceals, len)
                    }
                    helix_plugin::Decorations::VirtualLines(lines) => {
                        store.set_virtual_lines(layer, lines, len)
                    }
                    helix_plugin::Decorations::PatchHighlights(range, highlights) => {
                        store.patch_highlights(layer, range, highlights, len)
                    }
                    helix_plugin::Decorations::PatchAnnotations(range, annotations) => {
                        store.patch_annotations(layer, range, annotations, len)
                    }
                    helix_plugin::Decorations::RawOnCursorLine(raw) => {
                        store.set_raw_on_cursor_line(layer, raw)
                    }
                    helix_plugin::Decorations::RenderModes(modes) => {
                        let modes: Vec<_> = modes
                            .into_iter()
                            .map(|mode| match mode {
                                helix_plugin::Mode::Normal => helix_view::document::Mode::Normal,
                                helix_plugin::Mode::Insert => helix_view::document::Mode::Insert,
                                helix_plugin::Mode::Select => helix_view::document::Mode::Select,
                            })
                            .collect();
                        store.set_render_modes(layer, &modes)
                    }
                }
                helix_event::request_redraw();
            }
            Action::RunBackground {
                plugin,
                id,
                task,
                input,
            } => {
                // `done` runs on the worker thread: hand the result to the
                // editor's runtime.
                let runtime = tokio::runtime::Handle::current();
                let owner = plugin.clone();
                let done = Box::new(move |result| {
                    runtime.spawn(deliver_async(
                        owner,
                        Callback::BackgroundDone { id, result },
                    ));
                });
                with_host(|host| host.run_background(&plugin, id, task, input, done));
            }
            Action::StatusLine {
                plugin,
                segment,
                text,
            } => {
                let key = format!("{plugin}/{segment}");
                if text.is_empty() {
                    editor.plugin_statusline.remove(&key);
                } else {
                    editor.plugin_statusline.insert(key, text);
                }
                helix_event::request_redraw();
            }
            Action::OpenScratch {
                text,
                language,
                action,
                owner,
            } => {
                let action = editor_action(action);
                let existing = owner.as_ref().and_then(|(plugin, name)| {
                    editor
                        .documents()
                        .find(|doc| {
                            doc.plugin_owner.as_deref() == Some(plugin)
                                && doc.name.as_deref() == Some(name)
                        })
                        .map(|doc| doc.id())
                });
                let doc_id = match existing {
                    Some(doc_id) => {
                        editor.switch(doc_id, action);
                        doc_id
                    }
                    None => {
                        let doc_id = editor.new_file_with_text(action, &text);
                        let loader = editor.syn_loader.load();
                        let doc = editor.documents.get_mut(&doc_id).unwrap();
                        if let Some(language) = language {
                            if let Err(err) = doc.set_language_by_language_id(&language, &loader) {
                                log::warn!("plugin buffer: {err}");
                            }
                        }
                        if let Some((plugin, name)) = owner.clone() {
                            doc.name = Some(name.into());
                            doc.plugin_owner = Some(plugin);
                        }
                        doc_id
                    }
                };
                // Tell the owner the id of its buffer. Now, so what it does
                // (decorations) is in the first frame that shows the buffer;
                // later when this is itself the answer to that event.
                if let Some((plugin, _)) = owner {
                    let event = Event::DocumentOpen(plugin_doc_id(doc_id));
                    if OPENING.fetch_add(1, Ordering::Relaxed) == 0 {
                        let snapshot = snapshot(editor);
                        let result =
                            with_host(|host| host.dispatch_unsubscribed(&plugin, event, &snapshot));
                        if let Some(result) = result.flatten() {
                            report_with(editor, jobs.as_deref_mut(), &plugin, result);
                        }
                    } else {
                        later(move |editor, _| {
                            let snapshot = snapshot(editor);
                            let result = with_host(|host| {
                                host.dispatch_unsubscribed(&plugin, event, &snapshot)
                            });
                            if let Some(result) = result.flatten() {
                                report(editor, &plugin, result);
                            }
                        });
                    }
                    OPENING.fetch_sub(1, Ordering::Relaxed);
                }
            }
            Action::CreatePath { path, directory } => {
                if let Err(err) = editor.create_new_path(&path, directory) {
                    editor.set_error(format!("Failed to create {}: {err}", path.display()));
                }
            }
            Action::RenamePath { from, to } => {
                if let Err(err) = rename_path(editor, &from, &to) {
                    editor.set_error(err);
                }
            }
            Action::FileOps { plugin, id, ops } => {
                tokio::spawn(file_ops::run(plugin, id, ops));
            }
            Action::DeletePath(path) => {
                if let Err(err) = editor.delete_path(&path, true) {
                    editor.set_error(format!("Failed to delete {}: {err}", path.display()));
                }
            }
            Action::ShowSidebar {
                plugin,
                dock,
                title,
                items,
                width,
                selected,
                float,
            } => {
                let float = float.map(|(height, position)| {
                    use helix_plugin::v05::FloatPosition as P;
                    use helix_view::sidebar::FloatPosition as V;
                    let position = match position {
                        P::Center => V::Center,
                        P::Cursor => V::Cursor,
                        P::TopRight => V::TopRight,
                    };
                    (height, position)
                });
                let items: Vec<_> = items
                    .into_iter()
                    .map(|item| helix_view::sidebar::SidebarItem {
                        text: item.text,
                        scope: item.scope,
                        depth: item.depth,
                        command: item.command,
                    })
                    .collect();
                // The same plugin's panel keeps its scroll, focus and
                // (unless told otherwise) selection.
                let dock = view_dock(dock);
                let (scroll, focused, previous) = match editor.panel(dock) {
                    Some(old) if old.owner == plugin => (old.scroll, old.focused, old.selected),
                    _ => (0, false, 0),
                };
                let selected = selected
                    .unwrap_or(previous)
                    .min(items.len().saturating_sub(1));
                editor.set_panel(helix_view::sidebar::Sidebar {
                    dock,
                    owner: plugin,
                    title,
                    items,
                    width,
                    selected,
                    scroll,
                    focused,
                    float,
                    spans: Vec::new(),
                });
                helix_event::request_redraw();
            }
            Action::Window(action) => window_action(editor, action),
            Action::HistoryJump { doc, revision } => {
                let Some(doc) = editor_doc_id(editor, doc) else {
                    continue;
                };
                let Some(view) = view_for(editor, doc) else {
                    continue;
                };
                let (view, doc) = (
                    editor.tree.get_mut(view),
                    editor.documents.get_mut(&doc).expect("a view shows it"),
                );
                if doc.jump_to_revision(view, revision as usize) {
                    helix_event::request_redraw();
                }
            }
            Action::Fold {
                doc,
                first,
                last,
                fold,
            } => {
                if let Some(doc) =
                    editor_doc_id(editor, doc).and_then(|doc| editor.documents.get_mut(&doc))
                {
                    if fold {
                        doc.fold_lines(first as usize, last as usize);
                    } else {
                        doc.unfold_lines(first as usize, last as usize);
                    }
                    helix_event::request_redraw();
                }
            }
            Action::LanguageServer(action) => lsp_server::apply(editor, action),
            Action::Mouse { plugin, kinds } => mouse::listen(plugin, kinds),
            Action::DebugRequest {
                plugin,
                id,
                command,
                arguments,
            } => debug::request(editor, plugin, id, command, arguments),
            Action::SetBreakpoints { path, breakpoints } => {
                debug::set_breakpoints(editor, path, breakpoints)
            }
            Action::DebugListen { plugin, events } => debug::listen(plugin, events),
            Action::KeyModeDefine { plugin, def } => define_key_mode(editor, plugin, def),
            Action::KeyModeSwitch { name, doc, how } => {
                use crate::keymap::modes::Switch;
                let how = match how {
                    helix_plugin::v05::KeyModeSwitch::On => Switch::On,
                    helix_plugin::v05::KeyModeSwitch::Off => Switch::Off,
                    helix_plugin::v05::KeyModeSwitch::Toggle => Switch::Toggle,
                };
                let doc = match doc {
                    Some(doc) => editor_doc_id(editor, doc),
                    None => Some(editor.tree.get(editor.tree.focus).doc),
                };
                let language = doc
                    .and_then(|doc| editor.documents.get(&doc))
                    .and_then(|doc| doc.language_name().map(str::to_string));
                match crate::keymap::modes::switch(doc, language.as_deref(), &name, how) {
                    Ok((_, commands)) if !commands.is_empty() => {
                        crate::compositor::defer_ui(crate::compositor::DeferredUi::Callback(
                            Box::new(move |_compositor, cx| {
                                crate::commands::modes::run_commands(cx, &commands)
                            }),
                        ));
                        helix_event::request_redraw();
                    }
                    Ok(_) => helix_event::request_redraw(),
                    Err(err) => editor.set_error(format!("mode {name}: {err}")),
                }
            }
            #[cfg(unix)]
            Action::TerminalOpen {
                plugin,
                id,
                argv,
                split,
            } => {
                use helix_view::editor::Action as Open;
                let action = match split {
                    Some(true) => Open::HorizontalSplit,
                    Some(false) => Open::VerticalSplit,
                    None => Open::Replace,
                };
                match crate::terminal::open_as(editor, &argv, action, Some(id), Some(plugin)) {
                    // As `:terminal`: ready to type in.
                    Ok(_) => editor.mode = Mode::Insert,
                    Err(err) => editor.set_error(err.to_string()),
                }
            }
            #[cfg(unix)]
            Action::TerminalWrite { id, data } => {
                crate::terminal::write(id, data.as_bytes());
            }
            #[cfg(unix)]
            Action::TerminalClose(id) => crate::terminal::close_with_buffer(editor, id),
            #[cfg(not(unix))]
            Action::TerminalOpen { .. }
            | Action::TerminalWrite { .. }
            | Action::TerminalClose(_) => editor.set_error("terminals need a Unix system"),
            Action::Bar {
                plugin,
                kind,
                view,
                bar,
            } => set_bar(editor, plugin, kind, view, bar),
            Action::Canvas { doc, lines } => {
                if let Some(doc) =
                    editor_doc_id(editor, doc).and_then(|doc| editor.documents.get_mut(&doc))
                {
                    doc.plugin_canvas = lines;
                    helix_event::request_redraw();
                }
            }
            Action::StyleSpans {
                plugin,
                dock,
                spans,
            } => {
                let dock = view_dock(dock);
                if let Some(panel) = editor.panel_mut(dock).filter(|panel| panel.owner == plugin) {
                    panel.spans = spans;
                    panel
                        .spans
                        .sort_by_key(|(item, chars, _)| (*item, chars.start));
                    helix_event::request_redraw();
                }
            }
            Action::CloseSidebar { plugin, dock } => {
                let dock = view_dock(dock);
                if editor.panel(dock).is_some_and(|s| s.owner == plugin) {
                    editor.close_panel(dock);
                    helix_event::request_redraw();
                }
            }
            Action::FocusSidebar {
                plugin,
                dock,
                focused,
            } => {
                let dock = view_dock(dock);
                if editor.panel(dock).is_some_and(|s| s.owner == plugin) {
                    editor.focus_panel(dock, focused);
                    helix_event::request_redraw();
                }
            }
            Action::MarkSaved(doc) => {
                if let Some(doc) = editor_doc_id(editor, doc) {
                    let view = view_for(editor, doc);
                    let doc = editor.documents.get_mut(&doc).unwrap();
                    if let Some(view) = view {
                        doc.append_changes_to_history(editor.tree.get_mut(view));
                    }
                    doc.reset_modified();
                }
            }
            Action::SetRegister { name, values } => {
                if let Err(err) = editor.registers.write(name, values) {
                    editor.set_error(format!("plugin register {name}: {err}"));
                }
            }
            Action::Diagnostics {
                plugin,
                doc,
                diagnostics,
            } => {
                let Some(doc_id) = editor_doc_id(editor, doc) else {
                    continue;
                };
                let text = editor.documents[&doc_id].text().clone();
                let diagnostics = diagnostics
                    .into_iter()
                    .map(|d| to_lsp_diagnostic(&text, &plugin, d))
                    .collect();
                editor.set_plugin_diagnostics(doc_id, &plugin, diagnostics);
                helix_event::request_redraw();
            }
            Action::SetTimer { plugin, id, delay } => {
                let owner = plugin.clone();
                let task = tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    TIMERS
                        .lock()
                        .retain(|(timer_plugin, timer, _)| (timer_plugin, *timer) != (&plugin, id));
                    deliver_async(plugin, Callback::Timer { id }).await;
                });
                TIMERS.lock().push((owner, id, task.abort_handle()));
            }
            Action::CancelFiles { plugin, id } => file_ops::cancel(plugin, id),
            Action::CancelTimer { plugin, id } => {
                TIMERS.lock().retain(|(owner, timer, task)| {
                    let cancelled = (owner, *timer) == (&plugin, id);
                    if cancelled {
                        task.abort();
                    }
                    !cancelled
                });
            }
            Action::Completions { plugin, id, items } => {
                let mut waiting = COMPLETIONS.lock();
                if let Some(index) = waiting
                    .iter()
                    .position(|(owner, request, _)| *owner == plugin && *request == id)
                {
                    let (_, _, sender) = waiting.swap_remove(index);
                    let _ = sender.send(items);
                }
            }
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

fn view_dock(dock: helix_plugin::Dock) -> helix_view::sidebar::Dock {
    match dock {
        helix_plugin::Dock::Left => helix_view::sidebar::Dock::Left,
        helix_plugin::Dock::Right => helix_view::sidebar::Dock::Right,
        helix_plugin::Dock::Bottom => helix_view::sidebar::Dock::Bottom,
        helix_plugin::Dock::Float => helix_view::sidebar::Dock::Float,
    }
}

fn plugin_dock(dock: helix_view::sidebar::Dock) -> helix_plugin::Dock {
    match dock {
        helix_view::sidebar::Dock::Left => helix_plugin::Dock::Left,
        helix_view::sidebar::Dock::Right => helix_plugin::Dock::Right,
        helix_view::sidebar::Dock::Bottom => helix_plugin::Dock::Bottom,
        helix_view::sidebar::Dock::Float => helix_plugin::Dock::Float,
    }
}

fn editor_action(action: helix_plugin::OpenAction) -> helix_view::editor::Action {
    match action {
        helix_plugin::OpenAction::Replace => helix_view::editor::Action::Replace,
        helix_plugin::OpenAction::Hsplit => helix_view::editor::Action::HorizontalSplit,
        helix_plugin::OpenAction::Vsplit => helix_view::editor::Action::VerticalSplit,
    }
}

/// A plugin diagnostic as an LSP one, with UTF-32 (char) positions.
fn to_lsp_diagnostic(
    text: &helix_core::Rope,
    plugin: &str,
    diagnostic: helix_plugin::PluginDiagnostic,
) -> helix_lsp::lsp::Diagnostic {
    use helix_lsp::lsp;
    use helix_plugin::DiagnosticSeverity as S;
    let position = |char: usize| {
        let line = text.char_to_line(char);
        lsp::Position::new(line as u32, (char - text.line_to_char(line)) as u32)
    };
    lsp::Diagnostic {
        range: lsp::Range::new(
            position(diagnostic.range.start),
            position(diagnostic.range.end),
        ),
        severity: Some(match diagnostic.severity {
            S::Hint => lsp::DiagnosticSeverity::HINT,
            S::Info => lsp::DiagnosticSeverity::INFORMATION,
            S::Warning => lsp::DiagnosticSeverity::WARNING,
            S::Error => lsp::DiagnosticSeverity::ERROR,
        }),
        code: diagnostic.code.map(lsp::NumberOrString::String),
        source: Some(diagnostic.source.unwrap_or_else(|| plugin.to_string())),
        message: diagnostic.message,
        ..Default::default()
    }
}

/// Drop every plugin's diagnostics (the plugins were reloaded).
fn clear_plugin_diagnostics(editor: &mut Editor) {
    let plugins: std::collections::BTreeSet<String> = editor
        .diagnostics
        .values()
        .flatten()
        .filter_map(|(_, provider)| match provider {
            helix_core::diagnostic::DiagnosticProvider::Plugin { name } => Some(name.to_string()),
            _ => None,
        })
        .chain(editor.documents().flat_map(|doc| {
            doc.diagnostics().iter().filter_map(|d| match &d.provider {
                helix_core::diagnostic::DiagnosticProvider::Plugin { name } => {
                    Some(name.to_string())
                }
                _ => None,
            })
        }))
        .collect();
    for plugin in plugins {
        editor.clear_plugin_diagnostics(&plugin);
    }
}

/// Rename through the editor (language servers hear about it); buffers of
/// the file, or of files inside a renamed directory, follow.
fn rename_path(editor: &mut Editor, from: &Path, to: &Path) -> Result<(), String> {
    let inside: Vec<_> = editor
        .documents()
        .filter_map(|doc| {
            let rest = doc.path()?.strip_prefix(from).ok()?;
            (!rest.as_os_str().is_empty()).then(|| (doc.id(), to.join(rest)))
        })
        .collect();
    editor.move_path_no_replace(from, to).map_err(|err| {
        format!(
            "Failed to move {} to {}: {err}",
            from.display(),
            to.display()
        )
    })?;
    for (doc, path) in inside {
        editor.set_doc_path(doc, &path);
    }
    Ok(())
}

/// `files.apply` (API 0.4): operations in order, results to the plugin.
mod file_ops {
    use std::path::{Path, PathBuf};

    use helix_plugin::{Callback, FileOp};

    use crate::job;

    /// Batches run one at a time, so two can't race on the same paths.
    static BATCH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    /// Batches cancelled by their plugin: `(plugin, id)`.
    static CANCELLED: parking_lot::Mutex<Vec<(String, u32)>> = parking_lot::Mutex::new(Vec::new());

    /// Skip what batch `id` hasn't started yet.
    pub fn cancel(plugin: String, id: u32) {
        CANCELLED.lock().push((plugin, id));
    }

    pub async fn run(plugin: String, id: u32, ops: Vec<FileOp>) {
        let _turn = BATCH.lock().await;
        let cancelled = || {
            CANCELLED
                .lock()
                .iter()
                .any(|(owner, batch)| *owner == plugin && *batch == id)
        };
        let mut results = Vec::with_capacity(ops.len());
        for op in ops {
            if cancelled() {
                results.push(Err("cancelled".to_string()));
                continue;
            }
            if results.iter().any(Result::is_err) {
                results.push(Err("skipped after an earlier failure".to_string()));
                continue;
            }
            // Still allowed now, not only when it was asked for?
            let allowed = paths(&op).into_iter().try_for_each(|path| {
                super::with_host(|host| host.check_writable(&plugin, path))
                    .unwrap_or_else(|| Err("the plugins were unloaded".into()))
            });
            results.push(match allowed {
                Ok(()) => run_one(op).await,
                Err(err) => Err(err),
            });
        }
        CANCELLED
            .lock()
            .retain(|(owner, batch)| !(*owner == plugin && *batch == id));
        super::deliver_async(plugin, Callback::FilesDone { id, results }).await;
    }

    fn paths(op: &FileOp) -> Vec<&str> {
        match op {
            FileOp::Create { path, .. } | FileOp::Delete(path) | FileOp::Trash(path) => {
                vec![path]
            }
            FileOp::Rename { from, to } | FileOp::Copy { from, to } => vec![from, to],
        }
    }

    async fn run_one(op: FileOp) -> Result<(), String> {
        match op {
            // These go through the editor, which tells language servers and
            // moves buffers along.
            FileOp::Create { path, directory } => {
                on_editor(move |editor| {
                    let path = PathBuf::from(path);
                    if std::fs::symlink_metadata(&path).is_ok() {
                        return Err(format!("{} already exists", path.display()));
                    }
                    editor
                        .create_new_path(&path, directory)
                        .map_err(|err| format!("Failed to create {}: {err}", path.display()))
                })
                .await
            }
            FileOp::Rename { from, to } => {
                on_editor(move |editor| {
                    let to = PathBuf::from(to);
                    // `move_path` quietly does nothing for a missing file.
                    if std::fs::symlink_metadata(&from).is_err() {
                        return Err(format!("Failed to move {from}: it doesn't exist"));
                    }
                    if std::fs::symlink_metadata(&to).is_ok() {
                        return Err(format!("{} already exists", to.display()));
                    }
                    super::rename_path(editor, Path::new(&from), &to)
                })
                .await
            }
            FileOp::Delete(path) => {
                on_editor(move |editor| {
                    let path = PathBuf::from(path);
                    editor
                        .delete_path(&path, true)
                        .map_err(|err| format!("Failed to delete {}: {err}", path.display()))
                })
                .await
            }
            // These can take a while: off the editor's thread.
            FileOp::Copy { from, to } => {
                blocking(move || {
                    let to = PathBuf::from(to);
                    if std::fs::symlink_metadata(&to).is_ok() {
                        return Err(format!("{} already exists", to.display()));
                    }
                    copy(Path::new(&from), &to)
                        .map_err(|err| format!("Failed to copy {from} to {}: {err}", to.display()))
                })
                .await
            }
            FileOp::Trash(path) => {
                // Never a permanent delete: if the trash fails, so does this.
                blocking(move || {
                    trash::delete(&path).map_err(|err| format!("Failed to trash {path}: {err}"))
                })
                .await
            }
        }
    }

    async fn on_editor(
        f: impl FnOnce(&mut helix_view::Editor) -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        job::dispatch(move |editor, _| {
            let _ = sender.send(f(editor));
        })
        .await;
        receiver
            .await
            .unwrap_or_else(|_| Err("the editor stopped".into()))
    }

    async fn blocking(
        f: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        tokio::task::spawn_blocking(f)
            .await
            .unwrap_or_else(|err| Err(err.to_string()))
    }

    /// Copy a file or a directory tree; symlinks are copied as links.
    pub(super) fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
        let meta = std::fs::symlink_metadata(from)?;
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if meta.file_type().is_symlink() {
            #[cfg(unix)]
            return std::os::unix::fs::symlink(std::fs::read_link(from)?, to);
            #[cfg(not(unix))]
            return std::fs::copy(from, to).map(|_| ());
        }
        if meta.is_dir() {
            std::fs::create_dir(to)?;
            for entry in std::fs::read_dir(from)? {
                let entry = entry?;
                copy(&entry.path(), &to.join(entry.file_name()))?;
            }
            Ok(())
        } else if meta.is_file() {
            // Created here, so nothing that appeared meanwhile is replaced.
            let mut target = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(to)?;
            let mut source = std::fs::File::open(from)?;
            std::io::copy(&mut source, &mut target)?;
            target.set_permissions(meta.permissions())?;
            Ok(())
        } else {
            // FIFOs, sockets and devices would block or never end.
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} is not a regular file", from.display()),
            ))
        }
    }
}

runtime_local! {
    /// Keys a plugin without `allow-process = true` feeds are playing.
    static PROGRAMS_BLOCKED: AtomicBool = AtomicBool::new(false);
}

/// Whether commands that start programs are refused now: a plugin that
/// may not start them is feeding keys.
pub fn programs_blocked() -> bool {
    PROGRAMS_BLOCKED.load(Ordering::Relaxed)
}

/// Refuse program-starting commands while the guard lives, if `blocked`.
pub fn block_programs(blocked: bool) -> impl Drop {
    struct Guard(bool);
    impl Drop for Guard {
        fn drop(&mut self) {
            PROGRAMS_BLOCKED.store(self.0, Ordering::Relaxed);
        }
    }
    let before = PROGRAMS_BLOCKED.load(Ordering::Relaxed);
    PROGRAMS_BLOCKED.store(before || blocked, Ordering::Relaxed);
    Guard(before)
}

/// Whether running `command` (`:name args` or a static command) starts a
/// program: plugins need `allow-process = true` for those.
pub fn starts_program(command: &str) -> bool {
    // Typable commands by canonical name (aliases resolve), static by name.
    const TYPED: &[&str] = &[
        "run-shell-command",
        "pipe",
        "pipe-to",
        "insert-output",
        "append-output",
        "run-interactive",
        "run-and-wait",
        "compile",
        "grep",
    ];
    const STATIC: &[&str] = &[
        "shell_pipe",
        "shell_pipe_to",
        "shell_insert_output",
        "shell_append_output",
        "shell_keep_pipe",
        "run_and_wait",
        "compile",
    ];
    let command = command.trim();
    match command.strip_prefix(':') {
        Some(typed) => {
            let name = typed.split_whitespace().next().unwrap_or_default();
            // Any command's arguments can start a shell while they are
            // expanded (`:echo %sh{...}`).
            typed.contains("%sh")
                || crate::commands::typed::TYPABLE_COMMAND_MAP
                    .get(name)
                    .is_some_and(|cmd| TYPED.contains(&cmd.name))
        }
        None => STATIC.contains(&command.split_whitespace().next().unwrap_or_default()),
    }
}

/// `:write` on a plugin's buffer: the plugin handles it.
pub fn write_buffer(editor: &mut Editor, jobs: &mut Jobs, doc: DocumentId) -> anyhow::Result<()> {
    let Some(plugin) = editor
        .documents
        .get(&doc)
        .and_then(|d| d.plugin_owner.clone())
    else {
        anyhow::bail!("not a plugin buffer");
    };
    let snapshot = snapshot(editor);
    let actions = with_host(|host| host.buffer_write(&plugin, plugin_doc_id(doc), &snapshot))
        .ok_or_else(|| anyhow::anyhow!("plugins aren't loaded"))?;
    match actions {
        Ok(actions) => {
            apply(editor, Some(jobs), actions);
            Ok(())
        }
        Err(err) => {
            refresh_wanted(editor);
            Err(err)
        }
    }
}

/// Ask the completion-source plugins for completions at the primary cursor
/// of the current document. Each answer arrives on its receiver (dropped
/// when the plugin doesn't answer).
pub fn request_completions(
    editor: &mut Editor,
) -> Vec<(u16, tokio::sync::oneshot::Receiver<Vec<PluginCompletion>>)> {
    if !wants(EventKind::Completion) {
        return Vec::new();
    }
    let (view, doc) = current_ref!(editor);
    let position = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..)) as u32;
    let doc = plugin_doc_id(doc.id());
    let snapshot = snapshot(editor);
    let plugins = with_host(|host| {
        let names = host
            .plugins()
            .iter()
            .map(|p| p.name().to_owned())
            .collect::<Vec<_>>();
        host.subscribers(EventKind::Completion)
            .into_iter()
            .filter_map(|name| Some((names.iter().position(|n| *n == name)? as u16, name)))
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    let mut receivers = Vec::new();
    for (index, plugin) in plugins {
        let Some((id, result)) =
            with_host(|host| host.request_completion(&plugin, doc, position, &snapshot)).flatten()
        else {
            continue;
        };
        let (sender, receiver) = tokio::sync::oneshot::channel();
        {
            let mut waiting = COMPLETIONS.lock();
            // Only the latest request per plugin is answered.
            waiting.retain(|(owner, _, _)| *owner != plugin);
            waiting.push((plugin.clone(), id, sender));
        }
        report(editor, &plugin, result);
        receivers.push((index, receiver));
    }
    receivers
}

fn open_file_at(
    editor: &mut Editor,
    path: &Path,
    action: helix_plugin::OpenAction,
    line: Option<u32>,
    column: Option<u32>,
) -> anyhow::Result<()> {
    let action = editor_action(action);
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
            refresh_wanted(editor);
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
    // Pickers, prompts and next-key commands (`r`, `t`...) need the
    // compositor: the application sets them up right after this event.
    for callback in cx.callback {
        crate::compositor::defer_ui(crate::compositor::DeferredUi::Callback(callback));
    }
    if let Some((callback, kind)) = cx.on_next_key_callback {
        crate::compositor::defer_ui(crate::compositor::DeferredUi::NextKey(callback, kind));
    }
}

/// Register the hooks that turn editor events into plugin events.
/// A notification from language server `server`, for plugins listening.
pub fn lsp_notification(server: &str, method: &str, params: &helix_lsp::jsonrpc::Params) {
    let params = serde_json::to_value(params).unwrap_or(serde_json::Value::Null);
    lsp_server::notification(server, method, &params);
}

pub fn register_hooks() {
    lsp_server::install();
    register_hook!(move |event: &mut PostCommand<'_, '_>| {
        // Do not allocate a command name or lock anything unless requested.
        if wants(EventKind::PostCommand) && APPLYING.load(Ordering::Relaxed) == 0 {
            queue(Event::PostCommand(event.command.name().to_owned()));
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        // Lazy plugins for its language load first, so they hear of it.
        load_lazy_for(event.editor, event.doc);
        queue(Event::DocumentOpen(plugin_doc_id(event.doc)));
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if let Some(marks) = &*MARKS.lock() {
            marks
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .map(plugin_doc_id(event.doc.id()), event.changes);
        }
        if !event.ghost_transaction && APPLYING.load(Ordering::Relaxed) == 0 {
            queue(Event::DocumentChange(plugin_doc_id(event.doc.id())));
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        HISTORIES.lock().remove(&event.doc.id());
        LAYERS.lock().remove(&event.doc.id());
        if let Some(marks) = &*MARKS.lock() {
            marks
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .close(plugin_doc_id(event.doc.id()));
        }
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

fn refresh_wanted(editor: &mut Editor) {
    WANTED2.store(
        with_host(|host| wanted2_mask(host)).unwrap_or(0),
        Ordering::Relaxed,
    );
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
    WANTS_REGISTERS.store(
        with_host(|host| host.wants_registers()).unwrap_or(false),
        Ordering::Relaxed,
    );
    HAS_V05.store(
        with_host(|host| has_v05(host)).unwrap_or(false),
        Ordering::Relaxed,
    );
    debug::WANTED.store(
        with_host(|host| wants_debug(host)).unwrap_or(false),
        Ordering::Relaxed,
    );
    for plugin in disabled {
        tear_down(editor, &plugin);
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
    let (events, events2) = {
        let mut pending = PENDING.lock();
        pending.scheduled = false;
        (
            std::mem::take(&mut pending.events),
            std::mem::take(&mut pending.events2),
        )
    };
    for event in events {
        deliver_event(editor, event);
    }
    for (event, doc) in events2 {
        let snapshot = snapshot(editor);
        let Some(results) = with_host(|host| host.dispatch2(&event, doc, &snapshot)) else {
            continue;
        };
        for (plugin, result) in results {
            report(editor, &plugin, result);
        }
    }
}

/// [`report`] with jobs, for actions that need them.
fn report_with(
    editor: &mut Editor,
    jobs: Option<&mut Jobs>,
    _plugin: &str,
    result: anyhow::Result<Vec<Action>>,
) {
    match result {
        Ok(actions) => apply(editor, jobs, actions),
        Err(error) => {
            refresh_wanted(editor);
            editor.set_error(error.to_string());
        }
    }
}

/// Apply what a plugin call did, or show its error.
fn report(editor: &mut Editor, _plugin: &str, result: anyhow::Result<Vec<Action>>) {
    match result {
        Ok(actions) => apply(editor, None, actions),
        Err(error) => {
            refresh_wanted(editor);
            editor.set_error(error.to_string());
        }
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
    deliver_event_to(editor, event, plugins);
}

/// Handlers of one event may take this long together before the rest wait
/// for the editor to handle pending input.
const EVENT_BUDGET: Duration = Duration::from_millis(50);

fn deliver_event_to(editor: &mut Editor, event: Event, plugins: Vec<String>) {
    let started = std::time::Instant::now();
    let mut plugins = plugins.into_iter();
    while let Some(plugin) = plugins.next() {
        let snapshot = snapshot(editor);
        let result = with_host(|host| host.dispatch_to(&plugin, event.clone(), &snapshot));
        match result.flatten() {
            Some(Ok(actions)) => apply(editor, None, actions),
            Some(Err(error)) => {
                refresh_wanted(editor);
                editor.set_error(error.to_string());
            }
            None => {}
        }
        if started.elapsed() > EVENT_BUDGET {
            let rest: Vec<String> = plugins.collect();
            if !rest.is_empty() {
                let event = event.clone();
                later(move |editor, _| deliver_event_to(editor, event, rest));
            }
            return;
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
        if plugin.lazy {
            out.push_str(" — *not loaded yet: loads when needed*");
        }
        out.push('\n');
        let timing = plugin.timing;
        if timing.calls > 0 {
            out.push_str(&format!(
                "calls: {}, {:.2} ms on average, slowest {:.1} ms\n",
                timing.calls,
                timing.total.as_secs_f64() * 1000.0 / timing.calls as f64,
                timing.slowest.as_secs_f64() * 1000.0
            ));
        }
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
        if plugin
            .permissions
            .contains(&helix_plugin::Permission::Listen)
        {
            let ports: Vec<_> = plugin.listen.iter().map(u16::to_string).collect();
            if ports.is_empty() {
                out.push_str("listen: not granted\n");
            } else {
                out.push_str(&format!("listen: 127.0.0.1 port {}\n", ports.join(", ")));
            }
        }
        if let Some(keys) = &plugin.keys {
            let mut bound = Vec::new();
            fn walk(prefix: &str, table: &toml::Table, out: &mut Vec<String>) {
                for (key, value) in table {
                    let keys = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix} {key}")
                    };
                    match value {
                        toml::Value::Table(table) => walk(&keys, table, out),
                        value => out.push(format!("`{keys}` {}", value.as_str().unwrap_or("?"))),
                    }
                }
            }
            for (mode, table) in keys {
                let Some(table) = table.as_table() else {
                    continue;
                };
                let prefix = if mode == "normal" {
                    String::new()
                } else {
                    format!("({mode})")
                };
                walk(&prefix, table, &mut bound);
            }
            out.push_str(&format!("keys (if free): {}\n", bound.join(", ")));
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
    fn commands_that_start_programs() {
        for command in [
            ":sh ls",
            ":run-shell-command ls",
            ":pipe sort",
            ":| sort",
            ":insert-output date",
            ":bang ls",
            "shell_pipe",
            " :grep x",
            ":echo %sh{touch marker}",
            ":open \"%sh(echo file)\"",
            ":w %sh[x]",
        ] {
            assert!(starts_program(command), "{command}");
        }
        for command in [":write", ":echo hi", "goto_line_end", ":plugin-run x/y", ""] {
            assert!(!starts_program(command), "{command}");
        }
    }

    #[test]
    fn an_unreadable_lock_loads_no_plugins() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("plugins");
        let todos = Path::new(env!("CARGO_MANIFEST_DIR")).join("../helix-plugin/examples/todos");
        std::fs::create_dir_all(dir.join("todos")).unwrap();
        for file in ["plugin.toml", "plugin.wasm"] {
            std::fs::copy(todos.join(file), dir.join("todos").join(file)).unwrap();
        }
        std::fs::write(lock_file(&dir), "not = [valid").unwrap();
        let (host, _, errors) = build_host(&dir, &toml::Table::new());
        assert!(host.unwrap().plugins().is_empty());
        assert!(
            errors
                .iter()
                .any(|err| format!("{err:#}").contains("no plugins were loaded")),
            "{errors:?}"
        );
    }

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
