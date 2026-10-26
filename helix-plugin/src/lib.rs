//! WebAssembly plugin host for Helix.
//!
//! Plugins are [WebAssembly components](https://component-model.bytecodealliance.org)
//! implementing the `helix-plugin` world in `wit/v0.1/helix-plugin.wit`, run with
//! Wasmtime. Each plugin lives in its own directory with a `plugin.toml`
//! manifest and its `plugin.wasm`.
//!
//! This crate does not know about the editor: every call into a plugin gets a
//! [`Snapshot`] of the editor state, and everything the plugin wants to change
//! comes back as a list of [`Action`]s for the editor to apply. Plugins are
//! sandboxed: they only get the host functions their manifest asks for (see
//! [`Permission`]), no file system, network or environment access through
//! WASI, and every call runs with a time and memory limit. A plugin that traps
//! or runs out of time is disabled.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context as _};
use helix_core::{Range as CoreRange, Rope, Selection, Transaction};
use serde::Deserialize;
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Engine, Store, StoreLimits, StoreLimitsBuilder, Trap};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxView, WasiView};

mod api;
pub mod v05;

use api::{v0_1, v0_2, v0_3, v0_4, v0_5, Instance};
pub use types::{CommandInfo, Mode};
use v0_1::helix::plugin::types;

/// The newest supported plugin API. API 0.1 remains supported unchanged.
pub const API_VERSION: &str = "0.5";

pub type DocumentId = u32;

/// How often the epoch used for time limits advances.
const EPOCH_TICK: Duration = Duration::from_millis(5);

/// Capabilities a plugin can ask for in its manifest. Host functions that need
/// a permission the plugin didn't ask for fail with an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    /// Read document text, paths, languages and selections.
    ReadBuffers,
    /// Change document text and selections.
    EditBuffers,
    /// Open files.
    OpenFiles,
    /// Run editor commands.
    RunCommands,
    /// Run programs (`spawn`, `run-interactive`). Programs run outside the
    /// sandbox, so the user must also grant it, see [`ProcessGrant`].
    Process,
    /// Read (or write) files in the workspace through WASI. The user must
    /// also grant it, see [`FileGrant`].
    Filesystem,
    /// Read and write registers (API 0.3; the clipboard only write).
    Registers,
    /// Receive HTTP requests on 127.0.0.1 (API 0.4 `listener`). The user
    /// must also list the port in `allow-listen`.
    Listen,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::ReadBuffers => "read-buffers",
            Permission::EditBuffers => "edit-buffers",
            Permission::OpenFiles => "open-files",
            Permission::RunCommands => "run-commands",
            Permission::Process => "process",
            Permission::Filesystem => "filesystem",
            Permission::Registers => "registers",
            Permission::Listen => "listen",
        }
    }
}

/// Which programs the user lets a plugin run (`allow-process` in
/// `plugins.toml`). Asking for the `process` permission in `plugin.toml` is
/// not enough on its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ProcessGrant {
    #[default]
    None,
    /// Any program.
    Any,
    /// Only these programs, compared with the program (the first argument)
    /// exactly as the plugin passes it.
    Only(Vec<String>),
}

impl std::fmt::Display for ProcessGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "not granted"),
            Self::Any => write!(f, "granted"),
            Self::Only(programs) => write!(f, "only {}", programs.join(", ")),
        }
    }
}

/// Workspace file access the user grants a plugin (`allow-files` in
/// `plugins.toml`), on top of the `filesystem` permission.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FileGrant {
    #[default]
    None,
    Read,
    Write,
}

impl std::fmt::Display for FileGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "not granted",
            Self::Read => "read",
            Self::Write => "read and write",
        })
    }
}

/// What the user configured for a plugin in `plugins.toml`.
#[derive(Debug, Clone, Default)]
pub struct PluginSettings {
    /// Passed to `init`, as JSON.
    pub config: String,
    pub process: ProcessGrant,
    pub files: FileGrant,
    /// Directories besides the workspace that `files` applies to
    /// (`allow-dirs`), absolute.
    pub dirs: Vec<PathBuf>,
    /// Ports the plugin may listen on (`allow-listen`).
    pub listen: Vec<u16>,
}

impl PluginSettings {
    pub fn new(config: impl Into<String>) -> Self {
        Self {
            config: config.into(),
            ..Default::default()
        }
    }
}

/// Events a plugin can subscribe to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    DocumentOpen,
    DocumentChange,
    ModeSwitch,
    /// API 0.2 and later.
    DocumentSave,
    DocumentClose,
    SelectionChange,
    /// API 0.3 and later.
    PostCommand,
    FocusGained,
    FocusLost,
    EditorIdle,
    /// API 0.3: the plugin is a completion source.
    Completion,
    /// Not subscribable: `:write` on a plugin's buffer goes to its owner
    /// ([`PluginHost::buffer_write`]).
    BufferWrite,
}

/// What a [`Action::Decorate`] sets, validated against the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decorations {
    /// `(char range, theme scope)`.
    Highlights(Vec<(std::ops::Range<usize>, String)>),
    /// `(char position, text, theme scope)`.
    Annotations(Vec<(usize, String, String)>),
    /// `(char position, text drawn over the chars from there, theme scope)`.
    Overlays(Vec<(usize, String, String)>),
    /// `(char position of the line start, one-char text, theme scope)`.
    Signs(Vec<(usize, String, String)>),
    /// `(char position of the line start, theme scope)`.
    LineBackgrounds(Vec<(usize, String)>),
    /// `(char range within one line, replacement grapheme or empty, theme
    /// scope)`, sorted, not overlapping.
    Conceals(Vec<(std::ops::Range<usize>, String, String)>),
    /// Not a replacement: whether the layer is hidden on the cursor's line.
    RawOnCursorLine(bool),
    /// Not a replacement: the modes the layer is drawn in (empty: all).
    RenderModes(Vec<Mode>),
}

/// Most background tasks a plugin may have at once, and their default time
/// limit ([`Limits::background`]).
pub const MAX_BACKGROUND: usize = 4;
pub const BACKGROUND_LIMIT: Duration = Duration::from_secs(30);

/// Most running timers per plugin.
pub const MAX_TIMERS: usize = 16;
/// Most items in one completion answer.
pub const MAX_COMPLETIONS: usize = 1000;
/// Longest diagnostic message, in chars.
pub const MAX_DIAGNOSTIC_MESSAGE: usize = 4096;

/// How serious a [`PluginDiagnostic`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Hint,
    Info,
    Warning,
    Error,
}

/// A diagnostic from a plugin; char offsets, validated against the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDiagnostic {
    pub range: std::ops::Range<usize>,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub code: Option<String>,
    pub source: Option<String>,
}

/// Where a panel is (the sidebar is the left one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dock {
    Left,
    Right,
    Bottom,
    /// A floating panel over the editor (API 0.5 `floats`).
    Float,
}

/// A whole row of virtual text below a document line (API 0.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginVirtualLine {
    pub line: usize,
    pub text: String,
    pub scope: String,
}

/// One row of a plugin's sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarItem {
    pub text: String,
    pub scope: String,
    pub depth: u16,
    pub command: Option<(String, Vec<String>)>,
}

/// Most items in a sidebar.
pub const MAX_SIDEBAR_ITEMS: usize = 100_000;

/// The sidebar as a plugin call sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarSnapshot {
    pub dock: Dock,
    pub owner: String,
    pub selected: u32,
    pub focused: bool,
}

/// A file operation of `files.apply` (API 0.4); paths are checked against
/// the plugin's grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    Create { path: String, directory: bool },
    Rename { from: String, to: String },
    Copy { from: String, to: String },
    Delete(String),
    Trash(String),
}

/// Most operations in one `files.apply`.
pub const MAX_FILE_OPS: usize = 1000;
/// File operation batches of one plugin waiting or running at once.
pub const MAX_FILE_BATCHES: usize = 4;

/// A completion from a plugin. `replace_from` is a char offset, checked by
/// the editor against the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCompletion {
    pub label: String,
    pub insert: Option<String>,
    pub kind: Option<String>,
    pub documentation: Option<String>,
    pub replace_from: Option<usize>,
    /// `insert` (or `label`) is a snippet with tab stops (API 0.4
    /// `snippets.complete`).
    pub snippet: bool,
}

/// Optional features, for `requests.capabilities`.
pub const CAPABILITIES: &[&str] = &[
    "cancel-request",
    "revisions",
    "snippet-completions",
    "panels",
    "listen",
];

/// Most decorations in one layer.
pub const MAX_DECORATIONS: usize = 100_000;
/// Longest statusline segment, in chars.
pub const MAX_STATUSLINE: usize = 200;

/// Most programs a plugin may run at once.
pub const MAX_PROCESSES: usize = 16;
/// Most ports a plugin may listen on at once.
pub const MAX_LISTENERS: usize = 4;
/// Largest request body a listener accepts.
pub const MAX_LISTEN_BODY: usize = 4 << 20;
/// Default and largest time limits of a background program.
pub const DEFAULT_PROCESS_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_PROCESS_TIMEOUT: Duration = Duration::from_secs(600);
/// Most items in a plugin picker.
pub const MAX_PICKER_ITEMS: usize = 1_000_000;
/// Most columns in a plugin picker.
pub const MAX_PICKER_COLUMNS: usize = 8;

/// `plugin.toml`
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// The plugin API version the plugin was built for, see [`API_VERSION`].
    pub api: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    /// The component, relative to the plugin's directory.
    #[serde(default = "default_wasm")]
    pub wasm: PathBuf,
    /// Default key bindings, like `config.toml`'s `[keys]`, whose values are
    /// the plugin's command names. They only bind keys nothing else binds.
    #[serde(default)]
    pub keys: Option<toml::Table>,
}

fn default_wasm() -> PathBuf {
    PathBuf::from("plugin.wasm")
}

impl Manifest {
    /// API 0.3 or later (plugin buffers, background tasks, popups ids, ...).
    pub fn at_least_0_3(&self) -> bool {
        matches!(self.api.as_str(), "0.3" | "0.4" | "0.5")
    }

    /// API 0.5 or later (key callbacks, hooks, services...).
    pub fn at_least_0_5(&self) -> bool {
        self.api == "0.5"
    }

    pub fn parse(source: &str) -> anyhow::Result<Self> {
        let manifest: Manifest = toml::from_str(source)?;
        if !is_valid_name(&manifest.name) {
            bail!(
                "invalid plugin name '{}': use lowercase letters, digits, '-' and '_'",
                manifest.name
            );
        }
        if !matches!(manifest.api.as_str(), "0.1" | "0.2" | "0.3" | "0.4" | "0.5") {
            bail!(
                "plugin '{}' needs plugin API {} but this Pure supports 0.1 to 0.5",
                manifest.name,
                manifest.api
            );
        }
        // Checked before anything joins it to a directory: a file next to
        // plugin.toml, never `../x` or an absolute path.
        let mut components = manifest.wasm.components();
        if !matches!(
            (components.next(), components.next()),
            (Some(std::path::Component::Normal(_)), None)
        ) {
            bail!(
                "plugin '{}': wasm must be a file next to plugin.toml, not {}",
                manifest.name,
                manifest.wasm.display()
            );
        }
        Ok(manifest)
    }
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A document as a plugin sees it.
#[derive(Debug, Clone)]
pub struct DocumentSnapshot {
    pub text: Rope,
    pub path: Option<PathBuf>,
    pub language: Option<String>,
    /// Selection in the focused view on this document (or the first view
    /// showing it). `None` if no view shows the document.
    pub selection: Option<Selection>,
    /// Whether it has unsaved changes.
    pub modified: bool,
    /// `(plugin, name)` for a plugin's own buffer (`open-buffer`).
    pub owner: Option<(String, String)>,
    /// Changes with every change to the text.
    pub revision: u64,
}

/// The editor state a plugin call can read.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub documents: BTreeMap<DocumentId, DocumentSnapshot>,
    pub current: Option<DocumentId>,
    pub mode: Mode,
    pub workspace: Option<PathBuf>,
    pub cwd: PathBuf,
    pub config_dir: PathBuf,
    /// Register contents, only filled in when a plugin has the `registers`
    /// permission ([`PluginHost::wants_registers`]); no clipboard.
    pub registers: BTreeMap<char, Vec<String>>,
    /// The panels shown (the sidebar is the left one).
    pub panels: Vec<SidebarSnapshot>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            documents: BTreeMap::new(),
            current: None,
            mode: Mode::Normal,
            workspace: None,
            cwd: PathBuf::new(),
            config_dir: PathBuf::new(),
            registers: BTreeMap::new(),
            panels: Vec::new(),
        }
    }
}

/// A change a plugin asked for, to be applied by the editor in order.
#[derive(Debug, Clone)]
pub enum Action {
    /// Apply `transaction` to the document. Consecutive edits of one call to
    /// the same document should become one undo step.
    Edit {
        doc: DocumentId,
        transaction: Transaction,
    },
    SetSelection {
        doc: DocumentId,
        selection: Selection,
    },
    OpenFile(PathBuf),
    /// Open in a selected split with a 1-based position (API 0.3).
    OpenFileAt {
        path: PathBuf,
        action: OpenAction,
        line: Option<u32>,
        column: Option<u32>,
    },
    RunCommand(String),
    Status(String),
    Error(String),
    /// Run a program in the background and deliver
    /// [`Callback::ProcessDone`] when it exits.
    Spawn {
        plugin: String,
        id: u32,
        argv: Vec<String>,
        cwd: Option<PathBuf>,
        stdin: Option<String>,
        timeout: Duration,
        stream: bool,
    },
    /// Hand the terminal to a program, then deliver
    /// [`Callback::ProcessDone`].
    RunInteractive {
        plugin: String,
        id: u32,
        argv: Vec<String>,
        cwd: Option<PathBuf>,
        chooser_file: bool,
        wait_for_key: bool,
    },
    /// Kill a program started with `Spawn`.
    Kill {
        plugin: String,
        id: u32,
    },
    /// Listen on 127.0.0.1:`port`, delivering [`Callback::ListenRequest`]
    /// for each request and [`Callback::ListenFailed`] if listening fails.
    Listen {
        plugin: String,
        id: u32,
        port: u16,
    },
    /// Stop a listener started with `Listen`.
    StopListen {
        plugin: String,
        id: u32,
    },
    /// Show a picker and deliver [`Callback::PickerDone`].
    ShowPicker {
        plugin: String,
        id: u32,
        picker: PickerSpec,
        generation: Option<u64>,
    },
    /// Add items to an open picker.
    PushItems {
        plugin: String,
        id: u32,
        items: Vec<PickerItem>,
        generation: Option<u64>,
    },
    /// Replace the items of an open picker.
    SetItems {
        plugin: String,
        id: u32,
        items: Vec<PickerItem>,
        generation: Option<u64>,
    },
    /// Show markdown in a popup.
    Popup {
        markdown: String,
        centered: bool,
        owner: Option<(String, u32)>,
    },
    ClosePopup {
        plugin: String,
        id: u32,
    },
    /// Replace one of `plugin`'s decoration layers on `doc`.
    Decorate {
        plugin: String,
        doc: DocumentId,
        layer: String,
        decorations: Decorations,
    },
    /// Run `task` in the plugin's background instance
    /// ([`PluginHost::run_background`]) and deliver
    /// [`Callback::BackgroundDone`].
    RunBackground {
        plugin: String,
        id: u32,
        task: String,
        input: String,
    },
    /// Open a scratch buffer holding `text`; with `owner`, a buffer of
    /// that plugin with that name (shown instead if it exists).
    OpenScratch {
        text: String,
        language: Option<String>,
        action: OpenAction,
        owner: Option<(String, String)>,
    },
    /// Mark a plugin's buffer as saved.
    MarkSaved(DocumentId),
    /// Show or update `plugin`'s panel at `dock`; `selected: None` keeps it.
    ShowSidebar {
        plugin: String,
        dock: Dock,
        title: String,
        items: Vec<SidebarItem>,
        width: u16,
        selected: Option<usize>,
        /// A float's height (rows) and position.
        float: Option<(u16, v05::FloatPosition)>,
    },
    CloseSidebar {
        plugin: String,
        dock: Dock,
    },
    FocusSidebar {
        plugin: String,
        dock: Dock,
        focused: bool,
    },
    /// File operations through the editor (language servers and buffers
    /// follow). Paths are absolute and checked against the plugin's grant.
    CreatePath {
        path: PathBuf,
        directory: bool,
    },
    RenamePath {
        from: PathBuf,
        to: PathBuf,
    },
    DeletePath(PathBuf),
    /// Carry out `ops` (checked, absolute paths) in order, stopping at the
    /// first failure, and deliver [`Callback::FilesDone`].
    FileOps {
        plugin: String,
        id: u32,
        ops: Vec<FileOp>,
    },
    SetRegister {
        name: char,
        values: Vec<String>,
    },
    /// Replace `plugin`'s diagnostics on `doc`.
    Diagnostics {
        plugin: String,
        doc: DocumentId,
        diagnostics: Vec<PluginDiagnostic>,
    },
    /// Deliver [`Callback::Timer`] after `delay`.
    SetTimer {
        plugin: String,
        id: u32,
        delay: Duration,
    },
    /// Stop timer `id` started by an earlier call.
    CancelTimer {
        plugin: String,
        id: u32,
    },
    /// Skip the operations of files.apply batch `id` that haven't started.
    CancelFiles {
        plugin: String,
        id: u32,
    },
    /// The answer to completion request `id`
    /// ([`PluginHost::request_completion`]).
    Completions {
        plugin: String,
        id: u32,
        items: Vec<PluginCompletion>,
    },
    /// Set (or, empty, remove) a statusline segment.
    StatusLine {
        plugin: String,
        segment: String,
        text: String,
    },
    /// Ask for a line of text and deliver [`Callback::PromptDone`].
    ShowPrompt {
        plugin: String,
        id: u32,
        prompt: PromptSpec,
    },
}

/// Result of a background program or interactive chooser.
pub use v0_2::helix::plugin::process::Output as ProcessOutput;
/// How to open a picked item.
pub use v0_2::helix::plugin::types::OpenAction;
/// A picker choice, including the requested split action.
pub use v0_2::helix::plugin::ui::PickerResult;
/// A picker as a plugin describes it.
pub use v0_2::helix::plugin::ui::{
    Item as PickerItem, Picker as PickerSpec, Preview as PickerPreview, Prompt as PromptSpec,
};

/// The completion expected for an outstanding request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKind {
    Process,
    Picker,
    Prompt,
    Popup,
    Background,
    Timer,
    Completion,
    Files,
    Listen,
}

/// A result delivered after the original plugin call returned.
#[derive(Debug, Clone)]
pub enum Callback {
    /// A batch of stdout lines; the process request remains open.
    ProcessOutput {
        id: u32,
        chunk: String,
    },
    ProcessDone {
        id: u32,
        result: Result<ProcessOutput, String>,
    },
    PickerDone {
        id: u32,
        result: PickerResult,
    },
    PromptDone {
        id: u32,
        result: Option<String>,
    },
    /// What the user typed in a live picker. Unlike the others, it doesn't
    /// complete the request.
    PickerQuery {
        id: u32,
        query: String,
        generation: Option<u64>,
    },
    PopupClosed {
        id: u32,
    },
    /// What the plugin's background instance returned.
    BackgroundDone {
        id: u32,
        result: Result<String, String>,
    },
    Timer {
        id: u32,
    },
    /// One result per operation of a `files.apply` (API 0.4).
    FilesDone {
        id: u32,
        results: Vec<Result<(), String>>,
    },
    /// The body of a request a listener received; the listener stays open.
    /// Delivered as `on-process-output`.
    ListenRequest {
        id: u32,
        body: String,
    },
    /// The listener stopped (it couldn't bind, or accepting failed).
    /// Delivered as `on-process-done` with an error.
    ListenFailed {
        id: u32,
        error: String,
    },
}

impl Callback {
    fn request(&self) -> (u32, PendingKind) {
        match self {
            Self::ProcessDone { id, .. } | Self::ProcessOutput { id, .. } => {
                (*id, PendingKind::Process)
            }
            Self::PickerDone { id, .. } | Self::PickerQuery { id, .. } => {
                (*id, PendingKind::Picker)
            }
            Self::PromptDone { id, .. } => (*id, PendingKind::Prompt),
            Self::PopupClosed { id } => (*id, PendingKind::Popup),
            Self::BackgroundDone { id, .. } => (*id, PendingKind::Background),
            Self::Timer { id } => (*id, PendingKind::Timer),
            Self::FilesDone { id, .. } => (*id, PendingKind::Files),
            Self::ListenRequest { id, .. } | Self::ListenFailed { id, .. } => {
                (*id, PendingKind::Listen)
            }
        }
    }

    fn completes(&self) -> bool {
        !matches!(
            self,
            Self::PickerQuery { .. } | Self::ProcessOutput { .. } | Self::ListenRequest { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallContext {
    Init,
    Command,
    Event,
    Callback,
}

impl CallContext {
    fn allows_ui(self) -> bool {
        matches!(self, Self::Command | Self::Callback)
    }
}

// Never reuse an id, even after recreating the host on reload. Late results
// must not reach a new instance of a plugin with the same name.
static NEXT_REQUEST_ID: AtomicU32 = AtomicU32::new(1);

/// What happened in the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    DocumentOpen(DocumentId),
    DocumentChange(DocumentId),
    ModeSwitch {
        old: Mode,
        new: Mode,
    },
    DocumentSave(DocumentId),
    DocumentClose(DocumentId),
    SelectionChange(DocumentId),
    PostCommand(String),
    FocusGained,
    FocusLost,
    EditorIdle,
    /// API 0.3: `:write` on the plugin's own buffer.
    BufferWrite(DocumentId),
    /// API 0.3: answer with `complete(id, ...)`.
    Completion {
        id: u32,
        doc: DocumentId,
        position: u32,
    },
}

impl Event {
    pub fn kind(&self) -> EventKind {
        match self {
            Event::DocumentOpen(_) => EventKind::DocumentOpen,
            Event::DocumentChange(_) => EventKind::DocumentChange,
            Event::ModeSwitch { .. } => EventKind::ModeSwitch,
            Event::DocumentSave(_) => EventKind::DocumentSave,
            Event::DocumentClose(_) => EventKind::DocumentClose,
            Event::SelectionChange(_) => EventKind::SelectionChange,
            Event::PostCommand(_) => EventKind::PostCommand,
            Event::FocusGained => EventKind::FocusGained,
            Event::FocusLost => EventKind::FocusLost,
            Event::EditorIdle => EventKind::EditorIdle,
            Event::Completion { .. } => EventKind::Completion,
            Event::BufferWrite(_) => EventKind::BufferWrite,
        }
    }
}

/// Resource limits for plugin calls.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Maximum time for `init`.
    pub init: Duration,
    /// Maximum time for running a plugin command.
    pub command: Duration,
    /// Maximum time for an event handler.
    pub event: Duration,
    /// Maximum linear memory of a plugin, in bytes.
    pub memory: usize,
    /// Maximum time for a background task (`run-background`).
    pub background: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            init: Duration::from_secs(5),
            command: Duration::from_secs(2),
            event: Duration::from_millis(250),
            memory: 256 << 20,
            background: BACKGROUND_LIMIT,
        }
    }
}

/// Store data of one plugin.
struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    plugin: String,
    permissions: HashSet<Permission>,
    snapshot: Snapshot,
    actions: Vec<Action>,
    context: CallContext,
    next_id: &'static AtomicU32,
    pending: HashMap<u32, PendingKind>,
    picker_items: HashMap<u32, usize>,
    picker_generations: HashMap<u32, u64>,
    streaming: HashSet<u32>,
    process: ProcessGrant,
    files: FileGrant,
    /// The ports in `allow-listen`.
    listen: Vec<u16>,
    /// The directories `files` applies to: the workspace and `allow-dirs`.
    roots: Vec<PathBuf>,
    /// The plugin's own directory (`data-dir`), preopened read-write.
    data_dir: Option<PathBuf>,
    starts_program: Option<fn(&str) -> bool>,
    /// The latest `log` lines, for `:plugin-log`.
    log: std::collections::VecDeque<String>,
}

/// `log` lines kept per plugin.
const LOG_LINES: usize = 1000;
/// Longest line kept in a plugin's log.
const LOG_LINE_BYTES: usize = 4096;

impl State {
    /// For a background instance: no permissions, grants, files or snapshot.
    fn background(plugin: &str, memory: usize) -> Self {
        Self {
            wasi: WasiCtx::builder().build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new()
                .memory_size(memory)
                .instances(16)
                .tables(64)
                .memories(2)
                .build(),
            plugin: plugin.to_string(),
            permissions: HashSet::new(),
            snapshot: Snapshot::default(),
            actions: Vec::new(),
            context: CallContext::Init,
            next_id: &NEXT_REQUEST_ID,
            pending: HashMap::new(),
            picker_items: HashMap::new(),
            picker_generations: HashMap::new(),
            streaming: HashSet::new(),
            process: ProcessGrant::None,
            files: FileGrant::None,
            listen: Vec::new(),
            roots: Vec::new(),
            data_dir: None,
            starts_program: None,
            log: Default::default(),
        }
    }
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl State {
    fn request(&mut self, kind: PendingKind) -> Result<u32, String> {
        let id = self
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| "plugin request ids exhausted".to_string())?;
        self.pending.insert(id, kind);
        Ok(id)
    }

    fn need_ui(&self, function: &str) -> Result<(), String> {
        if self.context.allows_ui() {
            Ok(())
        } else {
            Err(format!(
                "{function} is only available from plugin commands and callbacks"
            ))
        }
    }

    fn need(&self, permission: Permission) -> Result<(), String> {
        if self.permissions.contains(&permission) {
            Ok(())
        } else {
            Err(format!(
                "permission denied: add \"{}\" to the permissions in plugin.toml",
                permission.as_str()
            ))
        }
    }

    fn document(&self, doc: DocumentId) -> Result<&DocumentSnapshot, String> {
        self.snapshot
            .documents
            .get(&doc)
            .ok_or_else(|| format!("no document with id {doc}"))
    }

    fn document_mut(&mut self, doc: DocumentId) -> Result<&mut DocumentSnapshot, String> {
        self.snapshot
            .documents
            .get_mut(&doc)
            .ok_or_else(|| format!("no document with id {doc}"))
    }
}

impl State {
    fn current_document(&mut self) -> Option<DocumentId> {
        self.snapshot.current
    }

    fn documents(&mut self) -> Vec<DocumentId> {
        self.snapshot.documents.keys().copied().collect()
    }

    fn current_mode(&mut self) -> Mode {
        self.snapshot.mode
    }

    fn document_path(&mut self, doc: DocumentId) -> Result<Option<String>, String> {
        self.need(Permission::ReadBuffers)?;
        Ok(self
            .document(doc)?
            .path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()))
    }

    fn document_language(&mut self, doc: DocumentId) -> Result<Option<String>, String> {
        self.need(Permission::ReadBuffers)?;
        Ok(self.document(doc)?.language.clone())
    }

    fn text(&mut self, doc: DocumentId) -> Result<String, String> {
        self.need(Permission::ReadBuffers)?;
        Ok(self.document(doc)?.text.to_string())
    }

    fn text_range(&mut self, doc: DocumentId, from: u32, to: u32) -> Result<String, String> {
        self.need(Permission::ReadBuffers)?;
        let text = &self.document(doc)?.text;
        let (from, to) = (from as usize, to as usize);
        if from > to || to > text.len_chars() {
            return Err(format!(
                "invalid range {from}..{to} for a document of {} chars",
                text.len_chars()
            ));
        }
        Ok(text.slice(from..to).to_string())
    }

    fn selections(&mut self, doc: DocumentId) -> Result<Vec<types::Range>, String> {
        self.need(Permission::ReadBuffers)?;
        let document = self.document(doc)?;
        let selection = document
            .selection
            .as_ref()
            .ok_or_else(|| format!("document {doc} is not shown in any view"))?;
        // Primary selection first.
        let primary = selection.primary_index();
        let ranges = std::iter::once(selection.primary())
            .chain(
                selection
                    .ranges()
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| i != primary)
                    .map(|(_, range)| *range),
            )
            .map(|range| types::Range {
                anchor: range.anchor as u32,
                head: range.head as u32,
            })
            .collect();
        Ok(ranges)
    }

    fn apply_edits(&mut self, doc: DocumentId, edits: Vec<types::Edit>) -> Result<(), String> {
        self.need(Permission::EditBuffers)?;
        let document = self.document_mut(doc)?;
        let transaction = transaction_from_edits(&document.text, edits)?;
        if transaction.changes().is_empty() {
            return Ok(());
        }
        // Later reads in this call see the edit (and its revision: the
        // editor counts one per transaction too).
        transaction.apply(&mut document.text);
        document.revision += 1;
        if let Some(selection) = document.selection.take() {
            document.selection = Some(selection.map(transaction.changes()));
        }
        self.actions.push(Action::Edit { doc, transaction });
        Ok(())
    }

    fn set_selections(&mut self, doc: DocumentId, ranges: Vec<types::Range>) -> Result<(), String> {
        self.need(Permission::EditBuffers)?;
        let document = self.document_mut(doc)?;
        if document.selection.is_none() {
            return Err(format!("document {doc} is not shown in any view"));
        }
        let selection = selection_from_ranges(&document.text, &ranges)?;
        document.selection = Some(selection.clone());
        self.actions.push(Action::SetSelection { doc, selection });
        Ok(())
    }

    fn open_file(&mut self, path: String) -> Result<(), String> {
        self.need(Permission::OpenFiles)?;
        if path.is_empty() {
            return Err("empty path".into());
        }
        self.actions.push(Action::OpenFile(PathBuf::from(path)));
        Ok(())
    }

    fn open_file_at(
        &mut self,
        path: String,
        action: OpenAction,
        line: Option<u32>,
        column: Option<u32>,
    ) -> Result<(), String> {
        self.need(Permission::OpenFiles)?;
        if path.is_empty() {
            return Err("empty path".into());
        }
        self.actions.push(Action::OpenFileAt {
            path: path.into(),
            action,
            line,
            column,
        });
        Ok(())
    }

    fn run_command(&mut self, command: String) -> Result<(), String> {
        self.need(Permission::RunCommands)?;
        if !self.context.allows_ui() {
            return Err(
                "run-command is only available while running a plugin command or callback".into(),
            );
        }
        if command.trim().is_empty() {
            return Err("empty command".into());
        }
        // A command can run any program (`:sh`): only with every program
        // allowed.
        if self.starts_program.is_some_and(|starts| starts(&command))
            && self.process != ProcessGrant::Any
        {
            return Err(format!(
                "'{}' runs programs: it needs allow-process = true in plugins.toml",
                command.trim()
            ));
        }
        self.actions.push(Action::RunCommand(command));
        Ok(())
    }

    fn set_status(&mut self, message: String) {
        self.actions.push(Action::Status(message));
    }

    fn set_error(&mut self, message: String) {
        self.actions.push(Action::Error(message));
    }

    fn log(&mut self, mut message: String) {
        // The log keeps lines, not floods: at most LOG_LINE_BYTES each.
        if message.len() > LOG_LINE_BYTES {
            let mut end = LOG_LINE_BYTES;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
            message.push('…');
        }
        log::info!("[plugin {}] {message}", self.plugin);
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(message);
    }

    /// Check that the plugin may run `argv` now.
    fn need_process(&self, argv: &[String]) -> Result<(), String> {
        self.need(Permission::Process)?;
        if self.context == CallContext::Init {
            return Err("programs can't be started from init".into());
        }
        let program = argv
            .first()
            .filter(|program| !program.is_empty())
            .ok_or("no program given")?;
        match &self.process {
            ProcessGrant::None => Err(format!(
                "process not granted: add `allow-process = true` (or a list of programs) \
                 under [{}] in plugins.toml",
                self.plugin
            )),
            ProcessGrant::Any => Ok(()),
            ProcessGrant::Only(programs) if programs.iter().any(|p| p == program) => Ok(()),
            ProcessGrant::Only(_) => Err(format!(
                "'{program}' is not in allow-process for [{}] in plugins.toml",
                self.plugin
            )),
        }?;
        let running = self
            .pending
            .values()
            .filter(|kind| **kind == PendingKind::Process)
            .count();
        if running >= MAX_PROCESSES {
            return Err(format!("at most {MAX_PROCESSES} programs can run at once"));
        }
        Ok(())
    }

    fn spawn(
        &mut self,
        argv: Vec<String>,
        cwd: Option<String>,
        stdin: Option<String>,
        timeout_ms: Option<u32>,
        stream: bool,
    ) -> Result<u32, String> {
        self.need_process(&argv)?;
        let timeout = timeout_ms
            .map(|ms| Duration::from_millis(ms.into()))
            .unwrap_or(DEFAULT_PROCESS_TIMEOUT)
            .min(MAX_PROCESS_TIMEOUT);
        let id = self.request(PendingKind::Process)?;
        if stream {
            self.streaming.insert(id);
        }
        self.actions.push(Action::Spawn {
            plugin: self.plugin.clone(),
            id,
            argv,
            cwd: cwd.map(PathBuf::from),
            stdin,
            timeout,
            stream,
        });
        Ok(id)
    }

    fn run_interactive(
        &mut self,
        argv: Vec<String>,
        cwd: Option<String>,
        chooser_file: bool,
        wait_for_key: bool,
    ) -> Result<u32, String> {
        self.need_ui("run-interactive")?;
        self.need_process(&argv)?;
        let id = self.request(PendingKind::Process)?;
        self.actions.push(Action::RunInteractive {
            plugin: self.plugin.clone(),
            id,
            argv,
            cwd: cwd.map(PathBuf::from),
            chooser_file,
            wait_for_key,
        });
        Ok(id)
    }

    fn kill(&mut self, id: u32) {
        if self.pending.get(&id) == Some(&PendingKind::Process) {
            self.actions.push(Action::Kill {
                plugin: self.plugin.clone(),
                id,
            });
        }
    }

    fn show_picker(&mut self, picker: PickerSpec, generations: bool) -> Result<u32, String> {
        self.need_ui("show-picker")?;
        if picker.columns.is_empty() || picker.columns.len() > MAX_PICKER_COLUMNS {
            return Err(format!(
                "a picker needs 1 to {MAX_PICKER_COLUMNS} columns, got {}",
                picker.columns.len()
            ));
        }
        if picker.items.len() > MAX_PICKER_ITEMS {
            return Err(format!(
                "a picker can have at most {MAX_PICKER_ITEMS} items"
            ));
        }
        let id = self.request(PendingKind::Picker)?;
        self.picker_items.insert(id, picker.items.len());
        let generation = generations.then_some(0);
        if let Some(generation) = generation {
            self.picker_generations.insert(id, generation);
        }
        self.actions.push(Action::ShowPicker {
            plugin: self.plugin.clone(),
            id,
            picker,
            generation,
        });
        Ok(id)
    }

    fn accepts_picker_items(&self, id: u32, generation: Option<u64>) -> Result<bool, String> {
        if self.pending.get(&id) != Some(&PendingKind::Picker) {
            return Err(format!("no open picker with id {id}"));
        }
        match (self.picker_generations.get(&id), generation) {
            (Some(current), Some(generation)) => Ok(*current == generation),
            (Some(_), None) => Err("live picker updates need a query generation".into()),
            (None, Some(_)) => Err("query generations require a live API 0.3 picker".into()),
            (None, None) => Ok(true),
        }
    }

    fn push_items(
        &mut self,
        id: u32,
        generation: Option<u64>,
        items: Vec<PickerItem>,
    ) -> Result<(), String> {
        if !self.accepts_picker_items(id, generation)? {
            return Ok(());
        }
        if self.context == CallContext::Init {
            return Err("push-items can't be used from init".into());
        }
        let count = self.picker_items.get(&id).copied().unwrap_or(0);
        let total = count
            .checked_add(items.len())
            .filter(|total| *total <= MAX_PICKER_ITEMS)
            .ok_or_else(|| format!("a picker can have at most {MAX_PICKER_ITEMS} items"))?;
        self.picker_items.insert(id, total);
        self.actions.push(Action::PushItems {
            plugin: self.plugin.clone(),
            id,
            items,
            generation,
        });
        Ok(())
    }

    fn set_items(
        &mut self,
        id: u32,
        generation: Option<u64>,
        items: Vec<PickerItem>,
    ) -> Result<(), String> {
        if !self.accepts_picker_items(id, generation)? {
            return Ok(());
        }
        if items.len() > MAX_PICKER_ITEMS {
            return Err(format!(
                "a picker can have at most {MAX_PICKER_ITEMS} items"
            ));
        }
        self.picker_items.insert(id, items.len());
        self.actions.push(Action::SetItems {
            plugin: self.plugin.clone(),
            id,
            items,
            generation,
        });
        Ok(())
    }

    /// Validate a layer name, a document and the number of decorations.
    fn need_layer(&self, doc: DocumentId, layer: &str, count: usize) -> Result<usize, String> {
        if layer.is_empty() || layer.len() > 64 || layer.contains('/') {
            return Err(format!("invalid layer name '{layer}': 1-64 chars, no '/'"));
        }
        if count > MAX_DECORATIONS {
            return Err(format!(
                "a layer can have at most {MAX_DECORATIONS} decorations"
            ));
        }
        Ok(self.document(doc)?.text.len_chars())
    }

    fn decorate(&mut self, doc: DocumentId, layer: String, decorations: Decorations) {
        self.actions.push(Action::Decorate {
            plugin: self.plugin.clone(),
            doc,
            layer,
            decorations,
        });
    }

    fn set_highlights(
        &mut self,
        doc: DocumentId,
        layer: String,
        highlights: Vec<(u32, u32, String)>,
    ) -> Result<(), String> {
        let len = self.need_layer(doc, &layer, highlights.len())?;
        let mut ranges = Vec::with_capacity(highlights.len());
        for (from, to, scope) in highlights {
            let (from, to) = (from as usize, to as usize);
            if from > to || to > len {
                return Err(format!(
                    "invalid range {from}..{to} for a document of {len} chars"
                ));
            }
            ranges.push((from..to, scope));
        }
        self.decorate(doc, layer, Decorations::Highlights(ranges));
        Ok(())
    }

    fn set_annotations(
        &mut self,
        doc: DocumentId,
        layer: String,
        annotations: Vec<(u32, String, String)>,
    ) -> Result<(), String> {
        let len = self.need_layer(doc, &layer, annotations.len())?;
        let mut checked = Vec::with_capacity(annotations.len());
        for (position, text, scope) in annotations {
            if position as usize > len {
                return Err(format!(
                    "invalid position {position} for a document of {len} chars"
                ));
            }
            if text.contains('\n') {
                return Err("virtual text can't contain line breaks".into());
            }
            checked.push((position as usize, text, scope));
        }
        self.decorate(doc, layer, Decorations::Annotations(checked));
        Ok(())
    }

    fn set_overlays(
        &mut self,
        doc: DocumentId,
        layer: String,
        overlays: Vec<(u32, String, String)>,
    ) -> Result<(), String> {
        let len = self.need_layer(doc, &layer, overlays.len())?;
        let text = &self.document(doc)?.text;
        let mut checked = Vec::with_capacity(overlays.len());
        for (position, overlay, scope) in overlays {
            let (from, count) = (position as usize, overlay.chars().count());
            if from + count > len {
                return Err(format!(
                    "invalid overlay at {position} for a document of {len} chars"
                ));
            }
            if text
                .slice(from..from + count)
                .chars()
                .any(|c| c == '\n' || c == '\r')
            {
                return Err("an overlay can't cover a line break".into());
            }
            checked.push((from, overlay, scope));
        }
        self.decorate(doc, layer, Decorations::Overlays(checked));
        Ok(())
    }

    fn set_signs(
        &mut self,
        doc: DocumentId,
        layer: String,
        signs: Vec<(u32, String, String)>,
    ) -> Result<(), String> {
        self.need_layer(doc, &layer, signs.len())?;
        let text = &self.document(doc)?.text;
        let lines = text.len_lines();
        let mut checked = Vec::with_capacity(signs.len());
        for (line, sign, scope) in signs {
            if line as usize >= lines {
                return Err(format!(
                    "invalid line {line} for a document of {lines} lines"
                ));
            }
            let sign: String = sign.chars().take(1).collect();
            checked.push((text.line_to_char(line as usize), sign, scope));
        }
        self.decorate(doc, layer, Decorations::Signs(checked));
        Ok(())
    }

    fn set_line_backgrounds(
        &mut self,
        doc: DocumentId,
        layer: String,
        lines: Vec<(u32, String)>,
    ) -> Result<(), String> {
        self.need_layer(doc, &layer, lines.len())?;
        let text = &self.document(doc)?.text;
        let count = text.len_lines();
        let mut checked = Vec::with_capacity(lines.len());
        for (line, scope) in lines {
            if line as usize >= count {
                return Err(format!(
                    "invalid line {line} for a document of {count} lines"
                ));
            }
            checked.push((text.line_to_char(line as usize), scope));
        }
        self.decorate(doc, layer, Decorations::LineBackgrounds(checked));
        Ok(())
    }

    fn run_background(&mut self, task: String, input: String) -> Result<u32, String> {
        if self.context == CallContext::Init {
            return Err("run-background can't be used from init".into());
        }
        let running = self
            .pending
            .values()
            .filter(|kind| **kind == PendingKind::Background)
            .count();
        if running >= MAX_BACKGROUND {
            return Err(format!(
                "at most {MAX_BACKGROUND} background tasks can run at once"
            ));
        }
        let id = self.request(PendingKind::Background)?;
        self.actions.push(Action::RunBackground {
            plugin: self.plugin.clone(),
            id,
            task,
            input,
        });
        Ok(id)
    }

    fn open_scratch(
        &mut self,
        text: String,
        language: Option<String>,
        action: OpenAction,
    ) -> Result<(), String> {
        self.need(Permission::OpenFiles)?;
        self.actions.push(Action::OpenScratch {
            text,
            language,
            action,
            owner: None,
        });
        Ok(())
    }

    fn open_buffer(
        &mut self,
        name: String,
        text: String,
        language: Option<String>,
        action: OpenAction,
    ) -> Result<(), String> {
        self.need(Permission::OpenFiles)?;
        if name.is_empty() || name.chars().count() > 256 || name.contains('\n') {
            return Err(format!(
                "invalid buffer name '{name}': 1-256 chars, one line"
            ));
        }
        self.actions.push(Action::OpenScratch {
            text,
            language,
            action,
            owner: Some((self.plugin.clone(), name)),
        });
        Ok(())
    }

    /// `path` if the plugin may write it: absolute, without `..`, inside the
    /// workspace or an `allow-dirs` directory, with `allow-files = "write"`.
    fn writable(&self, path: &str) -> Result<PathBuf, String> {
        self.need(Permission::Filesystem)?;
        if self.files != FileGrant::Write {
            return Err("file changes need allow-files = \"write\" in plugins.toml".into());
        }
        let path = PathBuf::from(path);
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(format!(
                "not an absolute, normalized path: {}",
                path.display()
            ));
        }
        let outside = || {
            Err(format!(
                "{} is outside the directories this plugin may change",
                path.display()
            ))
        };
        let Some(root) = self
            .roots
            .iter()
            .find(|root| path.starts_with(root) && **root != path)
        else {
            return outside();
        };
        // The editor changes the real file system: a symlinked directory on
        // the way must not lead outside the root.
        let real = |p: &Path| std::fs::canonicalize(p).ok();
        let parent = path.ancestors().skip(1).find(|a| a.exists());
        match (parent.and_then(real), real(root)) {
            (Some(parent), Some(root)) if parent.starts_with(&root) => Ok(path),
            _ => outside(),
        }
    }

    fn apply_files(&mut self, ops: Vec<FileOp>) -> Result<u32, String> {
        if self.context == CallContext::Init {
            return Err("files.apply can't be used from init".into());
        }
        if ops.len() > MAX_FILE_OPS {
            return Err(format!("at most {MAX_FILE_OPS} file operations at once"));
        }
        let running = self
            .pending
            .values()
            .filter(|kind| **kind == PendingKind::Files)
            .count();
        if running >= MAX_FILE_BATCHES {
            return Err(format!(
                "at most {MAX_FILE_BATCHES} files.apply batches can run at once"
            ));
        }
        let ops = ops
            .into_iter()
            .map(|op| {
                let check = |path: &str| {
                    self.writable(path)
                        .map(|path| path.to_string_lossy().into_owned())
                };
                Ok(match op {
                    FileOp::Create { path, directory } => FileOp::Create {
                        path: check(&path)?,
                        directory,
                    },
                    FileOp::Rename { from, to } => FileOp::Rename {
                        from: check(&from)?,
                        to: check(&to)?,
                    },
                    FileOp::Copy { from, to } => FileOp::Copy {
                        from: check(&from)?,
                        to: check(&to)?,
                    },
                    FileOp::Delete(path) => FileOp::Delete(check(&path)?),
                    FileOp::Trash(path) => FileOp::Trash(check(&path)?),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let id = self.request(PendingKind::Files)?;
        self.actions.push(Action::FileOps {
            plugin: self.plugin.clone(),
            id,
            ops,
        });
        Ok(id)
    }

    fn create_path(&mut self, path: String, directory: bool) -> Result<(), String> {
        let path = self.writable(&path)?;
        self.actions.push(Action::CreatePath { path, directory });
        Ok(())
    }

    fn rename_path(&mut self, from: String, to: String) -> Result<(), String> {
        let from = self.writable(&from)?;
        let to = self.writable(&to)?;
        self.actions.push(Action::RenamePath { from, to });
        Ok(())
    }

    fn delete_path(&mut self, path: String) -> Result<(), String> {
        let path = self.writable(&path)?;
        self.actions.push(Action::DeletePath(path));
        Ok(())
    }

    /// The directories this plugin may read (or write): the workspace and
    /// `allow-dirs`, if granted.
    fn file_roots(&self) -> Vec<String> {
        if !self.permissions.contains(&Permission::Filesystem) || self.files == FileGrant::None {
            return Vec::new();
        }
        self.roots
            .iter()
            .filter(|root| root.is_dir())
            .map(|root| root.to_string_lossy().into_owned())
            .collect()
    }

    fn show_sidebar(
        &mut self,
        dock: Dock,
        title: String,
        items: Vec<SidebarItem>,
        width: u32,
        selected: Option<u32>,
    ) -> Result<(), String> {
        self.show_panel(dock, title, items, width, selected, None)
    }

    /// Show a panel; `float` is a float's height and position.
    fn show_panel(
        &mut self,
        dock: Dock,
        title: String,
        items: Vec<SidebarItem>,
        width: u32,
        selected: Option<u32>,
        float: Option<(u16, v05::FloatPosition)>,
    ) -> Result<(), String> {
        if self.context == CallContext::Init {
            return Err("a panel can't be shown from init".into());
        }
        if items.len() > MAX_SIDEBAR_ITEMS {
            return Err(format!(
                "a sidebar can have at most {MAX_SIDEBAR_ITEMS} items"
            ));
        }
        let one_line = |text: String| -> String {
            text.chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect()
        };
        let items = items
            .into_iter()
            .map(|item| SidebarItem {
                text: one_line(item.text),
                ..item
            })
            .collect();
        // Columns beside the views, rows below them.
        let width = match dock {
            Dock::Bottom => width.clamp(3, 40),
            Dock::Left | Dock::Right => width.clamp(12, 80),
            Dock::Float => width.clamp(10, 200),
        } as u16;
        self.snapshot.panels.retain(|panel| panel.dock != dock);
        self.snapshot.panels.push(SidebarSnapshot {
            dock,
            owner: self.plugin.clone(),
            selected: selected.unwrap_or(0),
            focused: false,
        });
        self.actions.push(Action::ShowSidebar {
            plugin: self.plugin.clone(),
            dock,
            title: one_line(title),
            items,
            width,
            selected: selected.map(|s| s as usize),
            float,
        });
        Ok(())
    }

    fn close_sidebar(&mut self, dock: Dock) {
        self.actions.push(Action::CloseSidebar {
            plugin: self.plugin.clone(),
            dock,
        });
    }

    fn focus_sidebar(&mut self, dock: Dock, focused: bool) {
        self.actions.push(Action::FocusSidebar {
            plugin: self.plugin.clone(),
            dock,
            focused,
        });
    }

    fn sidebar_selected(&self, dock: Dock) -> Option<u32> {
        let panel = self
            .snapshot
            .panels
            .iter()
            .find(|panel| panel.dock == dock)?;
        (panel.owner == self.plugin).then_some(panel.selected)
    }

    fn buffer_name(&self, doc: DocumentId) -> Option<String> {
        let (owner, name) = self.snapshot.documents.get(&doc)?.owner.as_ref()?;
        (*owner == self.plugin).then(|| name.clone())
    }

    fn mark_saved(&mut self, doc: DocumentId) -> Result<(), String> {
        if self.buffer_name(doc).is_none() {
            return Err(format!(
                "document {doc} is not one of this plugin's buffers"
            ));
        }
        self.actions.push(Action::MarkSaved(doc));
        Ok(())
    }

    fn register(&self, name: char) -> Result<Vec<String>, String> {
        self.need(Permission::Registers)?;
        if matches!(name, '+' | '*') {
            return Err("plugins can't read the clipboard".into());
        }
        Ok(self
            .snapshot
            .registers
            .get(&name)
            .cloned()
            .unwrap_or_default())
    }

    fn set_register(&mut self, name: char, values: Vec<String>) -> Result<(), String> {
        self.need(Permission::Registers)?;
        if values.is_empty() {
            return Err("a register needs at least one value".into());
        }
        if !matches!(name, '+' | '*') {
            // Reads later in this call see it.
            self.snapshot.registers.insert(name, values.clone());
        }
        self.actions.push(Action::SetRegister { name, values });
        Ok(())
    }

    fn set_diagnostics(
        &mut self,
        doc: DocumentId,
        diagnostics: Vec<PluginDiagnostic>,
    ) -> Result<(), String> {
        let len = self.need_layer(doc, "diagnostics", diagnostics.len())?;
        for diagnostic in &diagnostics {
            if diagnostic.range.start > diagnostic.range.end || diagnostic.range.end > len {
                return Err(format!(
                    "invalid range {}..{} for a document of {len} chars",
                    diagnostic.range.start, diagnostic.range.end
                ));
            }
        }
        let diagnostics = diagnostics
            .into_iter()
            .map(|mut diagnostic| {
                if diagnostic.message.chars().count() > MAX_DIAGNOSTIC_MESSAGE {
                    diagnostic.message = diagnostic
                        .message
                        .chars()
                        .take(MAX_DIAGNOSTIC_MESSAGE)
                        .collect();
                }
                diagnostic
            })
            .collect();
        self.actions.push(Action::Diagnostics {
            plugin: self.plugin.clone(),
            doc,
            diagnostics,
        });
        Ok(())
    }

    fn set_timer(&mut self, delay_ms: u32) -> Result<u32, String> {
        if self.context == CallContext::Init {
            return Err("set-timer can't be used from init".into());
        }
        let running = self
            .pending
            .values()
            .filter(|kind| **kind == PendingKind::Timer)
            .count();
        if running >= MAX_TIMERS {
            return Err(format!("at most {MAX_TIMERS} timers can run at once"));
        }
        let id = self.request(PendingKind::Timer)?;
        self.actions.push(Action::SetTimer {
            plugin: self.plugin.clone(),
            id,
            delay: Duration::from_millis(delay_ms.max(1).into()),
        });
        Ok(id)
    }

    fn cancel_timer(&mut self, id: u32) {
        if self.pending.get(&id) == Some(&PendingKind::Timer) {
            self.pending.remove(&id);
            // Set in this same call: never start it. Otherwise stop it.
            let queued = self.actions.iter().position(
                |action| matches!(action, Action::SetTimer { id: set, .. } if *set == id),
            );
            match queued {
                Some(index) => {
                    self.actions.remove(index);
                }
                None => self.actions.push(Action::CancelTimer {
                    plugin: self.plugin.clone(),
                    id,
                }),
            }
        }
    }

    /// `listener.listen`: receive requests on 127.0.0.1:`port`.
    fn listen(&mut self, port: u16) -> Result<u32, String> {
        self.need(Permission::Listen)?;
        if !self.listen.contains(&port) {
            return Err(format!(
                "port {port} is not in allow-listen for [{}] in plugins.toml",
                self.plugin
            ));
        }
        let listening = self
            .pending
            .values()
            .filter(|kind| **kind == PendingKind::Listen)
            .count();
        if listening >= MAX_LISTENERS {
            return Err(format!("at most {MAX_LISTENERS} listeners can run at once"));
        }
        let id = self.request(PendingKind::Listen)?;
        self.actions.push(Action::Listen {
            plugin: self.plugin.clone(),
            id,
            port,
        });
        Ok(id)
    }

    fn document_revision(&mut self, doc: DocumentId) -> Result<u64, String> {
        self.need(Permission::ReadBuffers)?;
        Ok(self.document(doc)?.revision)
    }

    fn apply_edits_at(
        &mut self,
        doc: DocumentId,
        revision: u64,
        edits: Vec<types::Edit>,
    ) -> Result<(), String> {
        self.need(Permission::EditBuffers)?;
        let current = self.document(doc)?.revision;
        if current != revision {
            return Err(format!(
                "document {doc} changed: it is at revision {current}, not {revision}"
            ));
        }
        self.apply_edits(doc, edits)
    }

    /// `requests.cancel`: stop request `id`, whatever it is; its callback
    /// is never called.
    fn cancel_request(&mut self, id: u32) {
        let plugin = self.plugin.clone();
        match self.pending.get(&id).copied() {
            Some(PendingKind::Timer) => return self.cancel_timer(id),
            Some(PendingKind::Process) => {
                self.kill(id);
                self.streaming.remove(&id);
            }
            Some(PendingKind::Files) => self.actions.push(Action::CancelFiles { plugin, id }),
            Some(PendingKind::Listen) => self.actions.push(Action::StopListen { plugin, id }),
            Some(PendingKind::Completion) => self.actions.push(Action::Completions {
                plugin,
                id,
                items: Vec::new(),
            }),
            Some(PendingKind::Popup) => self.close_popup(id),
            Some(PendingKind::Picker) => {
                self.picker_items.remove(&id);
                self.picker_generations.remove(&id);
            }
            Some(PendingKind::Prompt | PendingKind::Background) => {}
            None => return,
        }
        self.pending.remove(&id);
    }

    fn complete(&mut self, id: u32, items: Vec<PluginCompletion>) -> Result<(), String> {
        if self.pending.get(&id) != Some(&PendingKind::Completion) {
            return Err(format!("no open completion request {id}"));
        }
        if items.len() > MAX_COMPLETIONS {
            return Err(format!("at most {MAX_COMPLETIONS} completions"));
        }
        self.pending.remove(&id);
        self.actions.push(Action::Completions {
            plugin: self.plugin.clone(),
            id,
            items,
        });
        Ok(())
    }

    fn set_raw_on_cursor_line(
        &mut self,
        doc: DocumentId,
        layer: String,
        raw: bool,
    ) -> Result<(), String> {
        self.need_layer(doc, &layer, 0)?;
        self.decorate(doc, layer, Decorations::RawOnCursorLine(raw));
        Ok(())
    }

    fn set_conceals(
        &mut self,
        doc: DocumentId,
        layer: String,
        conceals: Vec<(u32, u32, String, String)>,
    ) -> Result<(), String> {
        use helix_core::graphemes::{ensure_grapheme_boundary_next, ensure_grapheme_boundary_prev};
        use helix_core::unicode::segmentation::UnicodeSegmentation;

        let len = self.need_layer(doc, &layer, conceals.len())?;
        let text = self.document(doc)?.text.slice(..);
        let mut checked = Vec::with_capacity(conceals.len());
        for (from, to, replacement, scope) in conceals {
            let (from, to) = (from as usize, to as usize);
            if from > to || to > len {
                return Err(format!(
                    "invalid range {from}..{to} for a document of {len} chars"
                ));
            }
            // Whole graphemes, never past the end of the line.
            let from = ensure_grapheme_boundary_prev(text, from);
            let line = text.char_to_line(from);
            let line_end = helix_core::line_ending::line_end_char_index(&text, line);
            let to = ensure_grapheme_boundary_next(text, to).min(line_end);
            if from >= to {
                continue;
            }
            let replacement = replacement.graphemes(true).next().unwrap_or("").to_string();
            checked.push((from..to, replacement, scope));
        }
        checked.sort_by_key(|(range, ..)| (range.start, range.end));
        let mut end = 0;
        checked.retain(|(range, ..)| {
            let keep = range.start >= end;
            if keep {
                end = range.end;
            }
            keep
        });
        self.decorate(doc, layer, Decorations::Conceals(checked));
        Ok(())
    }

    fn set_render_modes(
        &mut self,
        doc: DocumentId,
        layer: String,
        modes: Vec<Mode>,
    ) -> Result<(), String> {
        self.need_layer(doc, &layer, 0)?;
        self.decorate(doc, layer, Decorations::RenderModes(modes));
        Ok(())
    }

    fn set_statusline(&mut self, segment: String, text: String) -> Result<(), String> {
        if segment.is_empty() || segment.len() > 64 || segment.contains('/') {
            return Err(format!(
                "invalid segment name '{segment}': 1-64 chars, no '/'"
            ));
        }
        // One line, bounded: it is drawn on every frame.
        let text: String = text
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(MAX_STATUSLINE)
            .collect();
        self.actions.push(Action::StatusLine {
            plugin: self.plugin.clone(),
            segment,
            text,
        });
        Ok(())
    }

    fn show_popup(&mut self, markdown: String) -> Result<(), String> {
        self.need_ui("show-popup")?;
        self.actions.push(Action::Popup {
            markdown,
            centered: false,
            owner: None,
        });
        Ok(())
    }

    fn show_popup_with_id(&mut self, markdown: String, centered: bool) -> Result<u32, String> {
        self.need_ui("show-popup")?;
        let id = self.request(PendingKind::Popup)?;
        self.actions.push(Action::Popup {
            markdown,
            centered,
            owner: Some((self.plugin.clone(), id)),
        });
        Ok(id)
    }

    fn close_popup(&mut self, id: u32) {
        if !self.context.allows_ui() {
            return;
        }
        if self.pending.get(&id) == Some(&PendingKind::Popup) {
            self.actions.push(Action::ClosePopup {
                plugin: self.plugin.clone(),
                id,
            });
        }
    }

    fn show_prompt(&mut self, prompt: PromptSpec) -> Result<u32, String> {
        self.need_ui("show-prompt")?;
        let id = self.request(PendingKind::Prompt)?;
        self.actions.push(Action::ShowPrompt {
            plugin: self.plugin.clone(),
            id,
            prompt,
        });
        Ok(id)
    }

    fn workspace_root(&mut self) -> Option<String> {
        self.snapshot
            .workspace
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
    }

    fn cwd(&mut self) -> String {
        self.snapshot.cwd.to_string_lossy().into_owned()
    }

    fn config_dir(&mut self) -> String {
        self.snapshot.config_dir.to_string_lossy().into_owned()
    }

    fn data_dir(&mut self) -> String {
        self.data_dir
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn document_modified(&mut self, doc: DocumentId) -> Result<bool, String> {
        self.need(Permission::ReadBuffers)?;
        Ok(self.document(doc)?.modified)
    }
}

/// Build a transaction from a plugin's edits, validating them against `text`.
fn transaction_from_edits(text: &Rope, mut edits: Vec<types::Edit>) -> Result<Transaction, String> {
    let len = text.len_chars();
    edits.sort_by_key(|edit| (edit.from, edit.to));
    let mut previous_end = 0;
    for edit in &edits {
        let (from, to) = (edit.from as usize, edit.to as usize);
        if from > to {
            return Err(format!("invalid edit {from}..{to}: start is after end"));
        }
        if to > len {
            return Err(format!(
                "invalid edit {from}..{to} for a document of {len} chars"
            ));
        }
        if from < previous_end {
            return Err(format!("edits overlap at {from}"));
        }
        previous_end = to;
    }
    Ok(Transaction::change(
        text,
        edits.into_iter().map(|edit| {
            let text = (!edit.text.is_empty()).then(|| edit.text.into());
            (edit.from as usize, edit.to as usize, text)
        }),
    ))
}

fn selection_from_ranges(text: &Rope, ranges: &[types::Range]) -> Result<Selection, String> {
    let len = text.len_chars();
    if ranges.is_empty() {
        return Err("a selection needs at least one range".into());
    }
    let ranges = ranges
        .iter()
        .map(|range| {
            if range.anchor as usize > len || range.head as usize > len {
                Err(format!(
                    "invalid range {}..{} for a document of {len} chars",
                    range.anchor, range.head
                ))
            } else {
                Ok(CoreRange::new(range.anchor as usize, range.head as usize))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Selection::new(ranges.into(), 0))
}

/// A loaded plugin.
pub struct Plugin {
    manifest: Manifest,
    dir: PathBuf,
    store: Store<State>,
    instance: Instance,
    commands: Vec<CommandInfo>,
    events: Vec<EventKind>,
    /// Why the plugin was disabled.
    disabled: Option<String>,
    ticker: Arc<Ticker>,
    runtime: Arc<CallRuntime>,
    /// For the background instance (API 0.3), made on first use.
    component: Component,
    worker: Option<std::sync::mpsc::Sender<BackgroundTask>>,
    /// How long its calls took.
    timing: CallTiming,
}

/// Calls to a plugin and their time, for `:plugin-list`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CallTiming {
    pub calls: u64,
    pub total: Duration,
    pub slowest: Duration,
}

/// A task for a plugin's background instance.
struct BackgroundTask {
    task: String,
    input: String,
    done: Box<dyn FnOnce(Result<String, String>) + Send>,
}

/// The thread running a plugin's background instance: a second instance of
/// the component with its own memory, no permissions, no snapshot (its host
/// calls see nothing and change nothing), and `limit` per task.
/// It ends when the plugin is dropped (the sender goes).
fn spawn_worker(
    engine: Engine,
    linker: Linker<State>,
    component: Component,
    ticker: Arc<Ticker>,
    manifest: &Manifest,
    memory: usize,
    limit: Duration,
) -> std::io::Result<std::sync::mpsc::Sender<BackgroundTask>> {
    let (name, api) = (manifest.name.clone(), manifest.api.clone());
    let (sender, tasks) = std::sync::mpsc::channel::<BackgroundTask>();
    std::thread::Builder::new()
        .name(format!("plugin-{name}-background"))
        .spawn(move || {
            let Ok(runtime) = CallRuntime::new() else {
                return;
            };
            let ticks = (limit.as_millis() / EPOCH_TICK.as_millis()).max(1) as u64;
            let mut instance: Option<(Store<State>, Instance)> = None;
            while let Ok(job) = tasks.recv() {
                // Tasks still queued when the plugins were reloaded aren't run.
                let Some(busy) = ticker.busy_unless_stopped() else {
                    (job.done)(Err("the plugins were reloaded".into()));
                    continue;
                };
                if instance.is_none() {
                    let mut store = Store::new(&engine, State::background(&name, memory));
                    store.limiter(|state| &mut state.limits);
                    store.set_epoch_deadline(ticks);
                    let started = runtime.run(limit, async {
                        if api == "0.3" {
                            v0_3::HelixPlugin::instantiate_async(&mut store, &component, &linker)
                                .await
                                .map(|instance| Instance::V0_3(Box::new(instance)))
                        } else if api == "0.4" {
                            v0_4::HelixPlugin::instantiate_async(&mut store, &component, &linker)
                                .await
                                .map(|instance| Instance::V0_4(Box::new(instance)))
                        } else {
                            v0_5::HelixPlugin::instantiate_async(&mut store, &component, &linker)
                                .await
                                .map(|instance| Instance::V0_5(Box::new(instance)))
                        }
                    });
                    match started {
                        Ok(started) => instance = Some((store, started)),
                        Err(err) => {
                            drop(busy);
                            (job.done)(Err(format!(
                                "the background instance failed to start: {err:#}"
                            )));
                            continue;
                        }
                    }
                }
                let (store, guest) = instance.as_mut().unwrap();
                store.set_epoch_deadline(ticks);
                let result =
                    runtime.run(limit, guest.call_background(store, &job.task, &job.input));
                drop(busy);
                // It has no editor access: what it asked for is dropped.
                store.data_mut().actions.clear();
                let result = match result {
                    Ok(result) => result,
                    Err(err) => {
                        // Start a fresh instance for the next task.
                        instance = None;
                        Err(match err.downcast_ref::<Trap>() {
                            Some(Trap::Interrupt) => format!(
                                "the background task took longer than {} ms",
                                limit.as_millis()
                            ),
                            _ => format!("the background task crashed: {err:#}"),
                        })
                    }
                };
                (job.done)(result);
            }
        })?;
    Ok(sender)
}

impl Plugin {
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn commands(&self) -> &[CommandInfo] {
        &self.commands
    }

    pub fn subscribes_to(&self, kind: EventKind) -> bool {
        self.events.contains(&kind)
    }

    /// Why the plugin was disabled, if it was.
    pub fn disabled(&self) -> Option<&str> {
        self.disabled.as_deref()
    }

    /// Run `f` with the store set up for one call: fresh snapshot, empty action
    /// queue and a deadline. A trap disables the plugin.
    fn call<R>(
        &mut self,
        snapshot: Snapshot,
        context: CallContext,
        limit: Duration,
        f: impl for<'a> FnOnce(&'a Instance, &'a mut Store<State>) -> CallFuture<'a, R> + Send,
    ) -> anyhow::Result<(R, Vec<Action>)>
    where
        R: Send + 'static,
    {
        if let Some(reason) = &self.disabled {
            bail!("plugin '{}' is disabled: {reason}", self.manifest.name);
        }
        let state = self.store.data_mut();
        state.snapshot = snapshot;
        state.actions.clear();
        state.context = context;
        let ticks = (limit.as_millis() / EPOCH_TICK.as_millis()).max(1) as u64;
        self.store.set_epoch_deadline(ticks);

        let guest = &self.instance;
        let store = &mut self.store;
        let busy = self.ticker.busy();
        let runtime = &self.runtime;
        let started = Instant::now();
        let result = outside_async_runtime(move || runtime.run(limit, f(guest, store)));
        drop(busy);
        let took = started.elapsed();
        self.timing.calls += 1;
        self.timing.total += took;
        self.timing.slowest = self.timing.slowest.max(took);
        let state = self.store.data_mut();
        let actions = std::mem::take(&mut state.actions);
        state.snapshot = Snapshot::default();
        state.context = CallContext::Init;
        match result {
            Ok(value) => Ok((value, actions)),
            Err(err) => {
                state.pending.clear();
                state.picker_items.clear();
                state.picker_generations.clear();
                state.streaming.clear();
                let reason = match err.downcast_ref::<Trap>() {
                    Some(Trap::Interrupt) => {
                        format!("it took longer than {} ms", limit.as_millis())
                    }
                    _ => format!("it crashed: {err:#}"),
                };
                log::error!("plugin '{}' disabled: {reason}", self.manifest.name);
                self.disabled = Some(reason.clone());
                Err(anyhow!(
                    "plugin '{}' was disabled because {reason}",
                    self.manifest.name
                ))
            }
        }
    }
}

/// Say what an instantiation error means: a component built for a newer
/// Pure (an import this one lacks) or for another API version.
fn explain_start_error(name: &str, api: &str, err: anyhow::Error) -> anyhow::Error {
    let message = format!("{err:#}");
    let quoted = |after: &str| {
        let rest = &message[message.find(after)? + after.len()..];
        let start = rest.find('`')? + 1;
        let end = start + rest[start..].find('`')?;
        Some(rest[start..end].to_string())
    };
    if message.contains("a matching implementation was not found in the linker") {
        if let Some(import) = quoted("component imports") {
            return anyhow!(
                "plugin '{name}' needs a newer Pure: it uses `{import}`, which this Pure \
                 doesn't have (update Pure, or use a version of the plugin for this one)"
            );
        }
    }
    if message.contains("no exported instance named `helix:plugin/") {
        return anyhow!(
            "plugin '{name}': plugin.toml says api = \"{api}\", but its component was built \
             for another API version"
        );
    }
    err.context(format!("failed to start plugin '{name}'"))
}

/// Information about a plugin, for listing.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub dir: PathBuf,
    pub permissions: Vec<Permission>,
    pub commands: Vec<CommandInfo>,
    pub disabled: Option<String>,
    pub process: ProcessGrant,
    pub files: FileGrant,
    /// The ports in `allow-listen`.
    pub listen: Vec<u16>,
    /// `[keys]` from `plugin.toml`.
    pub keys: Option<toml::Table>,
    pub timing: CallTiming,
}

/// Loads and runs plugins.
pub struct PluginHost {
    engine: Engine,
    linker: Linker<State>,
    plugins: Vec<Plugin>,
    limits: Limits,
    cache_dir: Option<PathBuf>,
    /// Shared with plugins granted `allow-files`.
    workspace: Option<PathBuf>,
    /// Plugins get `<data_root>/<name>` as their own directory.
    data_root: Option<PathBuf>,
    /// Whether an editor command starts a program (see
    /// [`PluginHost::set_program_check`]).
    starts_program: Option<fn(&str) -> bool>,
    ticker: Arc<Ticker>,
    runtime: Arc<CallRuntime>,
}

/// Advances the engine's epoch so plugin calls can be interrupted, but only
/// while a call runs: when no plugin runs, the thread sleeps and the editor
/// has no extra wake-ups.
struct Ticker {
    /// Plugin calls running.
    busy: AtomicUsize,
    stop: AtomicBool,
    thread: std::thread::Thread,
}

impl Ticker {
    fn start(engine: &Engine) -> anyhow::Result<Arc<Self>> {
        let engine = engine.weak();
        let (sender, receiver) = std::sync::mpsc::channel::<Arc<Ticker>>();
        let handle = std::thread::Builder::new()
            .name("helix-plugin-epoch".into())
            .spawn(move || {
                let Ok(ticker) = receiver.recv() else {
                    return;
                };
                // After the host is dropped, keep ticking until the calls
                // still running (background tasks) are interrupted.
                loop {
                    let stopping = ticker.stop.load(Ordering::SeqCst);
                    if ticker.busy.load(Ordering::SeqCst) == 0 {
                        if stopping {
                            break;
                        }
                        // Woken by `busy()` or on drop.
                        std::thread::park();
                        continue;
                    }
                    std::thread::sleep(EPOCH_TICK);
                    match engine.upgrade() {
                        Some(engine) => engine.increment_epoch(),
                        None => break,
                    }
                }
            })?;
        let ticker = Arc::new(Self {
            busy: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            thread: handle.thread().clone(),
        });
        let _ = sender.send(ticker.clone());
        Ok(ticker)
    }

    /// Tick until the returned guard is dropped.
    fn busy(self: &Arc<Self>) -> TickerGuard {
        self.busy.fetch_add(1, Ordering::SeqCst);
        self.thread.unpark();
        TickerGuard(self.clone())
    }

    /// Like `busy`, but `None` once the host is gone (the ticker may have
    /// stopped, so the call couldn't be interrupted).
    fn busy_unless_stopped(self: &Arc<Self>) -> Option<TickerGuard> {
        let guard = self.busy();
        (!self.stop.load(Ordering::SeqCst)).then_some(guard)
    }
}

struct TickerGuard(Arc<Ticker>);

impl Drop for TickerGuard {
    fn drop(&mut self) {
        self.0.busy.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        self.ticker.stop.store(true, Ordering::SeqCst);
        self.ticker.thread.unpark();
    }
}

impl PluginHost {
    /// `cache_dir` is where compiled plugins are cached to speed up startup.
    pub fn new(cache_dir: Option<PathBuf>) -> anyhow::Result<Self> {
        Self::with_limits(cache_dir, Limits::default())
    }

    pub fn with_limits(cache_dir: Option<PathBuf>, limits: Limits) -> anyhow::Result<Self> {
        let mut config = wasmtime::Config::new();
        config.epoch_interruption(true);
        config.async_support(true);
        config.wasm_component_model(true);
        let engine = Engine::new(&config)?;

        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        v0_1::HelixPlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        v0_2::HelixPlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        v0_3::HelixPlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        v0_4::HelixPlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        v0_5::HelixPlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;

        let runtime = Arc::new(CallRuntime::new()?);
        let ticker = Ticker::start(&engine)?;

        Ok(Self {
            engine,
            linker,
            plugins: Vec::new(),
            limits,
            cache_dir,
            workspace: None,
            data_root: None,
            starts_program: None,
            ticker,
            runtime,
        })
    }

    pub fn plugins(&self) -> &[Plugin] {
        &self.plugins
    }

    pub fn plugin(&self, name: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|plugin| plugin.name() == name)
    }

    pub fn info(&self) -> Vec<PluginInfo> {
        self.plugins
            .iter()
            .map(|plugin| PluginInfo {
                name: plugin.manifest.name.clone(),
                version: plugin.manifest.version.clone(),
                description: plugin.manifest.description.clone(),
                dir: plugin.dir.clone(),
                permissions: plugin.manifest.permissions.clone(),
                commands: plugin.commands.clone(),
                disabled: plugin.disabled.clone(),
                process: plugin.store.data().process.clone(),
                files: plugin.store.data().files,
                listen: plugin.store.data().listen.clone(),
                keys: plugin.manifest.keys.clone(),
                timing: plugin.timing,
            })
            .collect()
    }

    /// Remove all plugins.
    pub fn clear(&mut self) {
        self.plugins.clear();
    }

    /// The workspace plugins granted `allow-files` can access, and where
    /// plugins keep their own files. Set before loading: plugins see the
    /// directories as they were when they were loaded.
    pub fn set_dirs(&mut self, workspace: Option<PathBuf>, data_root: Option<PathBuf>) {
        self.workspace = workspace;
        self.data_root = data_root;
    }

    /// Tell which editor commands start programs (`:sh`, `:pipe`, ...): a
    /// plugin runs those (`run-command`) only with `allow-process = true`.
    /// Applies to plugins loaded afterwards.
    pub fn set_program_check(&mut self, starts_program: fn(&str) -> bool) {
        self.starts_program = Some(starts_program);
    }

    /// The workspace plugins with `allow-files` were given (see `set_dirs`).
    pub fn workspace(&self) -> Option<&Path> {
        self.workspace.as_deref()
    }

    /// Load every plugin in the subdirectories of `dir`. `settings` gives the
    /// settings for a plugin, or `None` if it is disabled. Returns the
    /// actions of the plugins' `init` calls and the errors of plugins that
    /// failed to load.
    pub fn load_all(
        &mut self,
        dir: &Path,
        mut settings: impl FnMut(&str) -> Option<PluginSettings>,
    ) -> (Vec<Action>, Vec<anyhow::Error>) {
        let mut actions = Vec::new();
        let mut errors = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return (actions, errors);
        };
        let mut dirs: Vec<_> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.join("plugin.toml").is_file())
            .collect();
        dirs.sort();
        let mut enabled = Vec::new();
        for dir in dirs {
            let manifest = match read_manifest(&dir) {
                Ok(manifest) => manifest,
                Err(err) => {
                    errors.push(err);
                    continue;
                }
            };
            if let Some(settings) = settings(&manifest.name) {
                enabled.push((dir, manifest, settings));
            }
        }
        // Compiling (on a cold cache) is the slow part: do it in parallel,
        // a few at a time, then load in order.
        let (engine, cache_dir) = (&self.engine, self.cache_dir.as_deref());
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
        let mut components = Vec::with_capacity(enabled.len());
        for batch in enabled.chunks(threads) {
            std::thread::scope(|scope| {
                let compiling: Vec<_> = batch
                    .iter()
                    .map(|(dir, manifest, _)| {
                        scope.spawn(move || {
                            compile(engine, cache_dir, &manifest.name, &dir.join(&manifest.wasm))
                        })
                    })
                    .collect();
                components.extend(compiling.into_iter().map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err(anyhow!("compiling the plugin panicked")))
                }));
            });
        }
        for ((dir, manifest, settings), component) in enabled.into_iter().zip(components) {
            match self.load_compiled(&dir, manifest, settings, component) {
                Ok(init_actions) => actions.extend(init_actions),
                Err(err) => errors.push(err),
            }
        }
        (actions, errors)
    }

    /// Load the plugin in `dir` and call its `init` with `config` (JSON).
    pub fn load(&mut self, dir: &Path, config: &str) -> anyhow::Result<Vec<Action>> {
        self.load_with(dir, PluginSettings::new(config))
    }

    /// Load the plugin in `dir` with the user's `settings`.
    pub fn load_with(
        &mut self,
        dir: &Path,
        settings: PluginSettings,
    ) -> anyhow::Result<Vec<Action>> {
        let manifest = read_manifest(dir)?;
        self.load_with_manifest(dir, manifest, settings)
    }

    /// WASI for a plugin: its own directory, and the workspace if granted.
    /// No arguments, environment, network or stdio.
    fn wasi(
        &self,
        name: &str,
        manifest: &Manifest,
        files: FileGrant,
        dirs: &[PathBuf],
    ) -> anyhow::Result<(WasiCtx, Option<PathBuf>)> {
        let mut wasi = WasiCtx::builder();
        wasi.allow_blocking_current_thread(false);
        let data_dir = match &self.data_root {
            Some(root) => {
                let dir = root.join(name);
                std::fs::create_dir_all(&dir)
                    .with_context(|| format!("failed to create {}", dir.display()))?;
                wasi.preopened_dir(
                    &dir,
                    dir.to_string_lossy(),
                    DirPerms::all(),
                    FilePerms::all(),
                )?;
                Some(dir)
            }
            None => None,
        };
        if manifest.permissions.contains(&Permission::Filesystem) && files != FileGrant::None {
            let (dir_perms, file_perms) = match files {
                FileGrant::Write => (DirPerms::all(), FilePerms::all()),
                _ => (DirPerms::READ, FilePerms::READ),
            };
            for dir in self.workspace.iter().chain(dirs) {
                if !dir.is_dir() {
                    continue;
                }
                wasi.preopened_dir(dir, dir.to_string_lossy(), dir_perms, file_perms)?;
            }
        }
        Ok((wasi.build(), data_dir))
    }

    fn load_with_manifest(
        &mut self,
        dir: &Path,
        manifest: Manifest,
        settings: PluginSettings,
    ) -> anyhow::Result<Vec<Action>> {
        let component = compile(
            &self.engine,
            self.cache_dir.as_deref(),
            &manifest.name,
            &dir.join(&manifest.wasm),
        );
        self.load_compiled(dir, manifest, settings, component)
    }

    fn load_compiled(
        &mut self,
        dir: &Path,
        manifest: Manifest,
        settings: PluginSettings,
        component: anyhow::Result<Component>,
    ) -> anyhow::Result<Vec<Action>> {
        let name = manifest.name.clone();
        if self.plugin(&name).is_some() {
            bail!("plugin '{name}' is already loaded");
        }
        let component = component.with_context(|| {
            format!(
                "failed to load plugin '{name}' from {}",
                dir.join(&manifest.wasm).display()
            )
        })?;

        let (wasi, data_dir) = self.wasi(&name, &manifest, settings.files, &settings.dirs)?;
        let state = State {
            wasi,
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.memory)
                .instances(16)
                .tables(64)
                .memories(2)
                .build(),
            plugin: name.clone(),
            permissions: manifest.permissions.iter().copied().collect(),
            snapshot: Snapshot::default(),
            actions: Vec::new(),
            context: CallContext::Init,
            next_id: &NEXT_REQUEST_ID,
            pending: HashMap::new(),
            picker_items: HashMap::new(),
            picker_generations: HashMap::new(),
            streaming: HashSet::new(),
            process: settings.process,
            files: settings.files,
            listen: settings.listen,
            roots: self
                .workspace
                .iter()
                .chain(&settings.dirs)
                .cloned()
                .collect(),
            data_dir,
            starts_program: self.starts_program,
            log: Default::default(),
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(
            (self.limits.init.as_millis() / EPOCH_TICK.as_millis()).max(1) as u64,
        );
        let linker = &self.linker;
        let busy = self.ticker.busy();
        let instance = outside_async_runtime(|| {
            self.runtime.run(self.limits.init, async {
                match manifest.api.as_str() {
                    "0.1" => v0_1::HelixPlugin::instantiate_async(&mut store, &component, linker)
                        .await
                        .map(Instance::V0_1),
                    "0.2" => v0_2::HelixPlugin::instantiate_async(&mut store, &component, linker)
                        .await
                        .map(Instance::V0_2),
                    "0.3" => v0_3::HelixPlugin::instantiate_async(&mut store, &component, linker)
                        .await
                        .map(|instance| Instance::V0_3(Box::new(instance))),
                    "0.4" => v0_4::HelixPlugin::instantiate_async(&mut store, &component, linker)
                        .await
                        .map(|instance| Instance::V0_4(Box::new(instance))),
                    "0.5" => v0_5::HelixPlugin::instantiate_async(&mut store, &component, linker)
                        .await
                        .map(|instance| Instance::V0_5(Box::new(instance))),
                    _ => unreachable!("manifest API was validated"),
                }
            })
        })
        .map_err(|err| explain_start_error(&name, &manifest.api, err))?;
        drop(busy);

        let mut plugin = Plugin {
            manifest,
            dir: dir.to_path_buf(),
            store,
            instance,
            commands: Vec::new(),
            events: Vec::new(),
            disabled: None,
            ticker: self.ticker.clone(),
            runtime: self.runtime.clone(),
            component: component.clone(),
            worker: None,
            timing: CallTiming::default(),
        };
        let config = settings.config;
        let (registration, actions) = plugin.call(
            Snapshot::default(),
            CallContext::Init,
            self.limits.init,
            move |guest, store| Box::pin(async move { guest.call_init(store, &config).await }),
        )?;
        let (commands, events) =
            registration.map_err(|err| anyhow!("plugin '{name}' failed to start: {err}"))?;
        for command in &commands {
            if !is_valid_name(&command.name) {
                bail!(
                    "plugin '{name}' registered an invalid command name '{}'",
                    command.name
                );
            }
        }
        plugin.commands = commands;
        plugin.events = events;
        plugin.events.dedup();
        self.plugins.push(plugin);
        Ok(actions)
    }

    /// All commands of enabled plugins, as `(plugin, command)`.
    pub fn commands(&self) -> Vec<(String, CommandInfo)> {
        self.plugins
            .iter()
            .filter(|plugin| plugin.disabled.is_none())
            .flat_map(|plugin| {
                plugin
                    .commands
                    .iter()
                    .map(|command| (plugin.manifest.name.clone(), command.clone()))
            })
            .collect()
    }

    /// Run `plugin`'s `command`. Returns the actions to apply, or an error if
    /// the command failed. The actions of a command that returns an error are
    /// discarded.
    pub fn run_command(
        &mut self,
        plugin: &str,
        command: &str,
        args: Vec<String>,
        snapshot: Snapshot,
    ) -> anyhow::Result<Vec<Action>> {
        let limit = self.limits.command;
        let plugin = self
            .plugins
            .iter_mut()
            .find(|p| p.manifest.name == plugin)
            .ok_or_else(|| anyhow!("no plugin named '{plugin}'"))?;
        if !plugin.commands.iter().any(|c| c.name == command) {
            bail!(
                "plugin '{}' has no command '{command}'",
                plugin.manifest.name
            );
        }
        let name = plugin.manifest.name.clone();
        let pending = plugin.store.data().pending.clone();
        let picker_items = plugin.store.data().picker_items.clone();
        let picker_generations = plugin.store.data().picker_generations.clone();
        let streaming = plugin.store.data().streaming.clone();
        let command_name = command.to_owned();
        let (result, actions) = plugin.call(
            snapshot,
            CallContext::Command,
            limit,
            move |guest, store| {
                Box::pin(async move { guest.call_run_command(store, &command_name, &args).await })
            },
        )?;
        if let Err(err) = result {
            plugin.store.data_mut().pending = pending;
            plugin.store.data_mut().picker_items = picker_items;
            plugin.store.data_mut().picker_generations = picker_generations;
            plugin.store.data_mut().streaming = streaming;
            bail!("{name}/{command}: {err}");
        }
        Ok(actions)
    }

    /// Reserve a result id for an API 0.2 or newer host operation, as the host
    /// functions do. For tests; ids are never reused after reload.
    #[doc(hidden)]
    pub fn register_request(&mut self, plugin: &str, kind: PendingKind) -> anyhow::Result<u32> {
        let plugin = self
            .plugins
            .iter_mut()
            .find(|p| p.name() == plugin)
            .ok_or_else(|| anyhow!("no plugin named '{plugin}'"))?;
        if !matches!(plugin.manifest.api.as_str(), "0.2" | "0.3" | "0.4" | "0.5")
            || plugin.disabled.is_some()
        {
            bail!("requests require an enabled API 0.2 or newer plugin");
        }
        if kind == PendingKind::Popup && !plugin.manifest.at_least_0_3() {
            bail!("popup requests require API 0.3");
        }
        plugin
            .store
            .data_mut()
            .request(kind)
            .map_err(anyhow::Error::msg)
    }

    /// The latest lines `plugin` wrote with `log` (oldest first).
    pub fn log_lines(&self, plugin: &str) -> Option<Vec<String>> {
        let plugin = self.plugin(plugin)?;
        Some(plugin.store.data().log.iter().cloned().collect())
    }

    /// Set which programs `plugin` may run (`allow-process` in
    /// `plugins.toml`). Plugins can't run programs from `init`, so this is set
    /// after loading.
    pub fn set_process_grant(&mut self, plugin: &str, grant: ProcessGrant) {
        if let Some(plugin) = self.plugins.iter_mut().find(|p| p.name() == plugin) {
            plugin.store.data_mut().process = grant;
        }
    }

    /// Whether deferred work still belongs to an enabled, outstanding request.
    pub fn has_request(&self, owner: &str, id: u32, kind: PendingKind) -> bool {
        self.plugins.iter().any(|plugin| {
            plugin.name() == owner
                && plugin.disabled.is_none()
                && plugin.store.data().pending.get(&id) == Some(&kind)
        })
    }

    /// Invalidate live results as soon as input changes, before debounce.
    /// This updates host bookkeeping without calling the guest.
    pub fn set_picker_generation(&mut self, owner: &str, id: u32, generation: u64) {
        let Some(plugin) = self
            .plugins
            .iter_mut()
            .find(|plugin| plugin.name() == owner && plugin.disabled.is_none())
        else {
            return;
        };
        let state = plugin.store.data_mut();
        if let Some(current) = state.picker_generations.get_mut(&id) {
            if *current != generation {
                *current = generation;
                state.picker_items.insert(id, 0);
            }
        }
    }

    /// Run `plugin`'s background request `id` in its background instance;
    /// `done` gets the result (on the worker thread). Starts the instance's
    /// thread on first use.
    pub fn run_background(
        &mut self,
        plugin: &str,
        id: u32,
        task: String,
        input: String,
        done: Box<dyn FnOnce(Result<String, String>) + Send>,
    ) {
        let (memory, limit) = (self.limits.memory, self.limits.background);
        let Some(plugin) = self
            .plugins
            .iter_mut()
            .find(|p| p.name() == plugin && p.disabled.is_none())
        else {
            return;
        };
        if plugin.store.data().pending.get(&id) != Some(&PendingKind::Background) {
            return;
        }
        if plugin.worker.is_none() {
            match spawn_worker(
                self.engine.clone(),
                self.linker.clone(),
                plugin.component.clone(),
                plugin.ticker.clone(),
                &plugin.manifest,
                memory,
                limit,
            ) {
                Ok(worker) => plugin.worker = Some(worker),
                Err(err) => return done(Err(format!("failed to start a worker: {err}"))),
            }
        }
        let job = BackgroundTask { task, input, done };
        if let Err(std::sync::mpsc::SendError(job)) = plugin.worker.as_ref().unwrap().send(job) {
            plugin.worker = None;
            (job.done)(Err("the background worker stopped".into()));
        }
    }

    /// Check again, just before it happens, that `plugin` may still change
    /// `path`: its grants or a directory on the way may have changed since
    /// it asked (a reload, a symlink swapped in).
    pub fn check_writable(&self, plugin: &str, path: &str) -> Result<(), String> {
        let plugin = self
            .plugins
            .iter()
            .find(|p| p.name() == plugin)
            .ok_or_else(|| format!("plugin '{plugin}' was unloaded"))?;
        if let Some(reason) = &plugin.disabled {
            return Err(format!("plugin '{}' was disabled: {reason}", plugin.name()));
        }
        plugin.store.data().writable(path).map(|_| ())
    }

    /// Deliver a completion with the command time limit and a fresh snapshot.
    /// Unknown, duplicate, wrong-kind and late results are ignored.
    pub fn callback(
        &mut self,
        plugin: &str,
        callback: Callback,
        snapshot: &Snapshot,
    ) -> anyhow::Result<Vec<Action>> {
        let Some(plugin) = self
            .plugins
            .iter_mut()
            .find(|p| p.name() == plugin && p.disabled.is_none())
        else {
            return Ok(Vec::new());
        };
        let (id, kind) = callback.request();
        if plugin.store.data().pending.get(&id) != Some(&kind) {
            return Ok(Vec::new());
        }
        if let Callback::PickerQuery { generation, .. } = &callback {
            if plugin.store.data().picker_generations.get(&id).copied() != *generation {
                return Ok(Vec::new());
            }
        }
        if matches!(callback, Callback::ProcessOutput { .. })
            && !plugin.store.data().streaming.contains(&id)
        {
            return Ok(Vec::new());
        }
        if callback.completes() {
            plugin.store.data_mut().pending.remove(&id);
            plugin.store.data_mut().picker_items.remove(&id);
            plugin.store.data_mut().picker_generations.remove(&id);
            plugin.store.data_mut().streaming.remove(&id);
        }
        plugin
            .call(
                snapshot.clone(),
                CallContext::Callback,
                self.limits.command,
                move |guest, store| {
                    Box::pin(async move { guest.call_callback(store, &callback).await })
                },
            )
            .map(|((), actions)| actions)
    }

    /// Send `event` to the plugins that subscribed to it. Returns the actions
    /// of each plugin, or the error if a plugin failed (and was disabled).
    pub fn dispatch(
        &mut self,
        event: Event,
        snapshot: &Snapshot,
    ) -> Vec<(String, anyhow::Result<Vec<Action>>)> {
        self.subscribers(event.kind())
            .into_iter()
            .filter_map(|name| {
                let result = self.dispatch_to(&name, event.clone(), snapshot)?;
                Some((name, result))
            })
            .collect()
    }

    /// The enabled plugins subscribed to events of `kind`.
    pub fn subscribers(&self, kind: EventKind) -> Vec<String> {
        self.plugins
            .iter()
            .filter(|plugin| plugin.disabled.is_none() && plugin.events.contains(&kind))
            .map(|plugin| plugin.manifest.name.clone())
            .collect()
    }

    /// Send `event` to `plugin` only (if it is enabled and subscribed), so
    /// the editor can apply each plugin's actions before the next plugin
    /// sees the state.
    pub fn dispatch_to(
        &mut self,
        plugin: &str,
        event: Event,
        snapshot: &Snapshot,
    ) -> Option<anyhow::Result<Vec<Action>>> {
        let kind = event.kind();
        self.dispatch_if(plugin, event, snapshot, |p| p.events.contains(&kind))
    }

    /// [`Self::dispatch_to`] whether or not the plugin subscribed (e.g. the
    /// open event of a buffer it created). API 0.3 plugins only.
    pub fn dispatch_unsubscribed(
        &mut self,
        plugin: &str,
        event: Event,
        snapshot: &Snapshot,
    ) -> Option<anyhow::Result<Vec<Action>>> {
        self.dispatch_if(plugin, event, snapshot, |p| p.manifest.at_least_0_3())
    }

    fn dispatch_if(
        &mut self,
        plugin: &str,
        event: Event,
        snapshot: &Snapshot,
        wanted: impl Fn(&Plugin) -> bool,
    ) -> Option<anyhow::Result<Vec<Action>>> {
        let limit = self.limits.event;
        let plugin = self
            .plugins
            .iter_mut()
            .find(|p| p.manifest.name == plugin && p.disabled.is_none() && wanted(p))?;
        Some(
            plugin
                .call(
                    snapshot.clone(),
                    CallContext::Event,
                    limit,
                    move |guest, store| Box::pin(guest.call_event(store, event)),
                )
                .map(|((), actions)| actions),
        )
    }

    /// Ask `plugin` for completions at `position`: a new request replaces
    /// the plugin's open one. Returns the request id and the actions of the
    /// event call (the answer may be among them, or come later).
    pub fn request_completion(
        &mut self,
        plugin: &str,
        doc: DocumentId,
        position: u32,
        snapshot: &Snapshot,
    ) -> Option<(u32, anyhow::Result<Vec<Action>>)> {
        let state = self
            .plugins
            .iter_mut()
            .find(|p| p.name() == plugin && p.disabled.is_none())?
            .store
            .data_mut();
        state
            .pending
            .retain(|_, kind| *kind != PendingKind::Completion);
        let id = state.request(PendingKind::Completion).ok()?;
        let result = self.dispatch_to(plugin, Event::Completion { id, doc, position }, snapshot)?;
        Some((id, result))
    }

    /// `:write` on `plugin`'s buffer `doc`: call its `on-buffer-write` as a
    /// command (it may show prompts).
    pub fn buffer_write(
        &mut self,
        plugin: &str,
        doc: DocumentId,
        snapshot: &Snapshot,
    ) -> anyhow::Result<Vec<Action>> {
        let limit = self.limits.command;
        let plugin = self
            .plugins
            .iter_mut()
            .find(|p| p.name() == plugin && p.disabled.is_none() && p.manifest.at_least_0_3())
            .ok_or_else(|| anyhow!("the plugin owning this buffer isn't running"))?;
        plugin
            .call(
                snapshot.clone(),
                CallContext::Command,
                limit,
                move |guest, store| Box::pin(guest.call_event(store, Event::BufferWrite(doc))),
            )
            .map(|((), actions)| actions)
    }

    /// Drop an unanswered request (e.g. a completion the editor stopped
    /// waiting for).
    pub fn forget_request(&mut self, owner: &str, id: u32, kind: PendingKind) {
        if let Some(plugin) = self.plugins.iter_mut().find(|p| p.name() == owner) {
            let pending = &mut plugin.store.data_mut().pending;
            if pending.get(&id) == Some(&kind) {
                pending.remove(&id);
            }
        }
    }

    /// Whether snapshots need register contents.
    pub fn wants_registers(&self) -> bool {
        self.plugins.iter().any(|plugin| {
            plugin.disabled.is_none()
                && plugin.manifest.permissions.contains(&Permission::Registers)
        })
    }

    /// Whether any enabled plugin listens to events of `kind`.
    pub fn wants(&self, kind: EventKind) -> bool {
        self.plugins
            .iter()
            .any(|plugin| plugin.disabled.is_none() && plugin.events.contains(&kind))
    }
}

/// Compile a component, using the cache when possible.
fn compile(
    engine: &Engine,
    cache_dir: Option<&Path>,
    name: &str,
    wasm: &Path,
) -> anyhow::Result<Component> {
    let bytes = std::fs::read(wasm)?;
    let cache_file = cache_dir.map(|dir| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        engine.precompile_compatibility_hash().hash(&mut hasher);
        dir.join(format!("{name}-{:016x}.cwasm", hasher.finish()))
    });
    if let Some(cache_file) = &cache_file {
        // Read into memory rather than `deserialize_file`, which maps the
        // file: if the file was changed in place while mapped (by another
        // program, or an older Helix), accessing the code would crash.
        if let Ok(cached) = std::fs::read(cache_file) {
            // SAFETY: the file was written by `Component::serialize` below
            // for this engine configuration (its name includes a hash of
            // the configuration) and lives in the user's own cache
            // directory. Wasmtime still checks the header and version.
            match unsafe { Component::deserialize(engine, &cached) } {
                Ok(component) => {
                    prune_cache(cache_file, name);
                    return Ok(component);
                }
                Err(err) => {
                    log::warn!("ignoring cached plugin {}: {err:#}", cache_file.display())
                }
            }
        }
    }
    let component = Component::new(engine, &bytes)?;
    if let Some(cache_file) = &cache_file {
        let write = || -> anyhow::Result<()> {
            std::fs::create_dir_all(cache_file.parent().unwrap())?;
            // Write to a temporary file first so a crash can't leave a
            // truncated file behind.
            static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let tmp = cache_file.with_extension(format!(
                "tmp{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let temporary = CacheTemp(tmp);
            std::fs::write(&temporary.0, component.serialize()?)?;
            std::fs::rename(&temporary.0, cache_file)?;
            prune_cache(cache_file, name);
            Ok(())
        };
        if let Err(err) = write() {
            log::warn!("failed to cache plugin '{name}': {err:#}");
        }
    }
    Ok(component)
}

/// Remove obsolete compiled versions of this plugin after a successful
/// load/write. Loading reads the compiled bytes into memory, so another
/// editor using an older version does not retain a mapping of these files.
fn prune_cache(current: &Path, name: &str) {
    let Some(parent) = current.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let prefix = format!("{name}-");
    for entry in entries.flatten() {
        let filename = entry.file_name();
        let Some(hash) = filename
            .to_str()
            .and_then(|name| name.strip_prefix(&prefix))
            .and_then(|name| name.strip_suffix(".cwasm"))
        else {
            continue;
        };
        if hash.len() == 16
            && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            && entry.path() != current
        {
            if let Err(error) = std::fs::remove_file(entry.path()) {
                log::debug!("could not remove obsolete plugin cache: {error}");
            }
        }
    }
}

/// Clean up an unfinished cache write on serialization, I/O or rename errors.
struct CacheTemp(PathBuf);
impl Drop for CacheTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

type CallFuture<'a, R> = Pin<Box<dyn Future<Output = wasmtime::Result<R>> + Send + 'a>>;

/// One parked runtime per host. Only a plugin call drives its timer and I/O.
/// Shutdown must not wait for an uncancellable OS operation (e.g. opening a
/// FIFO): WASI runs those on Tokio's blocking pool, outside the editor thread.
struct CallRuntime(Option<tokio::runtime::Runtime>);

impl CallRuntime {
    fn new() -> anyhow::Result<Self> {
        Ok(Self(Some(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        )))
    }

    fn run<R>(
        &self,
        limit: Duration,
        future: impl Future<Output = wasmtime::Result<R>>,
    ) -> wasmtime::Result<R> {
        self.0.as_ref().unwrap().block_on(async {
            tokio::time::timeout(limit, future)
                .await
                .map_err(|_| anyhow!(Trap::Interrupt))?
        })
    }
}

impl Drop for CallRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

/// Run a bounded async plugin call outside the editor's entered runtime.
/// The current-thread runtime advances only while this scoped thread runs.
fn outside_async_runtime<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    std::thread::scope(|scope| match scope.spawn(f).join() {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    })
}

fn read_manifest(dir: &Path) -> anyhow::Result<Manifest> {
    let path = dir.join("plugin.toml");
    let source = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Manifest::parse(&source).with_context(|| format!("invalid {}", path.display()))
}

#[cfg(test)]
mod test;
