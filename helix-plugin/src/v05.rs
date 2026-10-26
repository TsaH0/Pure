//! API 0.5 on the host side: the types the editor sees, and the `State`
//! methods behind the 0.5 imports.

use crate::{DocumentId, Mode, State};

/// Where a plugin key binding sits among the keymaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyLayer {
    /// Above the user's keys (needs `allow-key-override`).
    Override,
    /// Above Pure's defaults, below the user's keys.
    Plugin,
    /// Only keys nothing else binds.
    Fallback,
}

/// A key bound to a plugin callback (`keymaps.set`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding {
    pub mode: Mode,
    pub keys: String,
    pub name: String,
    pub description: String,
    pub layer: KeyLayer,
    pub document: Option<DocumentId>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    Handled,
    PassThrough,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    Proceed,
    Cancel(String),
}

/// The completion menu, as plugins see it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuState {
    pub items: Vec<String>,
    pub selected: Option<u32>,
    pub previewing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Select(Option<u32>),
    Next,
    Previous,
    Confirm,
    Abort,
    Close,
}

/// What `subscriptions.subscribe` subscribes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubscriptionEvent {
    DocumentOpen,
    DocumentChange,
    DocumentSave,
    DocumentClose,
    SelectionChange,
    ModeSwitch,
    PostCommand,
    FocusGained,
    FocusLost,
    EditorIdle,
    BufferEnter,
    BufferLeave,
    ViewEnter,
    ViewLeave,
    CompletionMenu,
    ConfigChange,
    PluginChange,
    Shutdown,
    PreCommand,
    PreSave,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventFilter {
    pub language: Option<String>,
    pub path_glob: Option<String>,
    pub document: Option<DocumentId>,
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    Native,
    Typable,
    Plugin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEntry {
    pub name: String,
    pub doc: String,
    pub kind: CommandKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Command {
        name: String,
        args: Vec<String>,
        count: Option<u32>,
    },
    SetMode(Mode),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewInfo {
    pub id: u64,
    pub document: DocumentId,
    /// x, y, width, height.
    pub area: (u16, u16, u16, u16),
    pub first_line: u32,
    pub last_line: u32,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiState {
    pub mode: Mode,
    pub completion_menu: bool,
    pub prompt: bool,
    pub picker: bool,
    pub popup: bool,
    pub focused_panel: Option<String>,
    pub pending_keys: String,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            completion_menu: false,
            prompt: false,
            picker: false,
            popup: false,
            focused_panel: None,
            pending_keys: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInfo {
    pub plugin: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatPosition {
    Center,
    Cursor,
    TopRight,
}

#[derive(Debug, Clone)]
pub struct FloatSpec {
    pub title: String,
    pub items: Vec<crate::SidebarItem>,
    pub width: u32,
    pub height: u32,
    pub position: FloatPosition,
    pub selected: Option<u32>,
    pub focus: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gravity {
    Before,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Aborted,
    Confirmed,
    Dismissed,
    ModeChange,
    Replaced,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionEvent {
    Opened(u32),
    Updated(u32),
    Confirmed(String),
    Closed(CloseReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    Loaded,
    Unloaded,
    Disabled,
}

/// The `events2.on-event` events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event2 {
    BufferEnter(DocumentId, u64),
    BufferLeave(DocumentId, u64),
    ViewEnter(u64),
    ViewLeave(u64),
    CompletionMenu(CompletionEvent),
    ConfigChange(String),
    PluginChange(String, PluginState),
    FileChange(u32, Vec<String>),
    Shutdown,
}

/// Results of 0.5 requests (`callbacks2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callback2 {
    CommandDone {
        id: u32,
        result: Result<(), String>,
    },
    BatchDone {
        id: u32,
        results: Vec<Result<(), String>>,
    },
    ServiceReply {
        id: u32,
        result: Result<String, String>,
    },
    LspReply {
        id: u32,
        result: Result<String, String>,
    },
}

fn not_yet<T>(what: &str) -> Result<T, String> {
    Err(format!("{what} is not available yet"))
}

impl State {
    pub(crate) fn set_key(&mut self, _binding: KeyBinding) -> Result<(), String> {
        not_yet("keymaps.set")
    }
    pub(crate) fn unset_key(
        &mut self,
        _mode: Mode,
        _keys: String,
        _document: Option<DocumentId>,
    ) -> Result<(), String> {
        not_yet("keymaps.unset")
    }
    pub(crate) fn list_keys(&mut self) -> Vec<KeyBinding> {
        Vec::new()
    }
    pub(crate) fn menu_state(&mut self) -> Option<MenuState> {
        None
    }
    pub(crate) fn menu_action(&mut self, _action: MenuAction) -> Result<(), String> {
        not_yet("completion-menu")
    }
    pub(crate) fn subscribe(
        &mut self,
        _event: SubscriptionEvent,
        _filter: EventFilter,
    ) -> Result<u32, String> {
        not_yet("subscriptions.subscribe")
    }
    pub(crate) fn unsubscribe(&mut self, _id: u32) {}
    pub(crate) fn command_list(&mut self) -> Vec<CommandEntry> {
        Vec::new()
    }
    pub(crate) fn execute(
        &mut self,
        _name: String,
        _args: Vec<String>,
        _count: Option<u32>,
    ) -> Result<u32, String> {
        not_yet("commands.execute")
    }
    pub(crate) fn batch(&mut self, _steps: Vec<Step>) -> Result<u32, String> {
        not_yet("commands.batch")
    }
    pub(crate) fn set_mode(&mut self, _mode: Mode) -> Result<(), String> {
        not_yet("commands.set-mode")
    }
    pub(crate) fn views(&mut self) -> Vec<ViewInfo> {
        Vec::new()
    }
    pub(crate) fn ui_state(&mut self) -> UiState {
        UiState::default()
    }
    pub(crate) fn register_service(
        &mut self,
        _name: String,
        _version: String,
        _callers: Vec<String>,
    ) -> Result<(), String> {
        not_yet("services.register")
    }
    pub(crate) fn discover_services(&mut self, _name: &str) -> Vec<ServiceInfo> {
        Vec::new()
    }
    pub(crate) fn call_service(
        &mut self,
        _service: String,
        _version: String,
        _method: String,
        _payload: String,
    ) -> Result<u32, String> {
        not_yet("services.call")
    }
    pub(crate) fn show_float(&mut self, float: FloatSpec) -> Result<(), String> {
        let height = float.height.clamp(3, 100) as u16;
        self.show_panel(
            crate::Dock::Float,
            float.title,
            float.items,
            float.width,
            float.selected,
            Some((height, float.position)),
        )?;
        if float.focus {
            self.focus_sidebar(crate::Dock::Float, true);
        }
        Ok(())
    }
    pub(crate) fn set_mark(
        &mut self,
        _doc: DocumentId,
        _position: u32,
        _gravity: Gravity,
    ) -> Result<u64, String> {
        not_yet("marks.set")
    }
    pub(crate) fn get_mark(&mut self, _id: u64) -> Option<(DocumentId, u32)> {
        None
    }
    pub(crate) fn remove_mark(&mut self, _id: u64) {}
    pub(crate) fn list_marks(&mut self, _doc: DocumentId) -> Vec<(u64, u32)> {
        Vec::new()
    }
    pub(crate) fn set_virtual_lines(
        &mut self,
        _doc: DocumentId,
        _layer: String,
        _lines: Vec<crate::PluginVirtualLine>,
    ) -> Result<(), String> {
        not_yet("decorations2.set-virtual-lines")
    }
    pub(crate) fn patch_highlights(
        &mut self,
        _doc: DocumentId,
        _layer: String,
        _from: u32,
        _to: u32,
        _highlights: Vec<(u32, u32, String)>,
    ) -> Result<(), String> {
        not_yet("decorations2.patch-highlights")
    }
    pub(crate) fn patch_annotations(
        &mut self,
        _doc: DocumentId,
        _layer: String,
        _from: u32,
        _to: u32,
        _annotations: Vec<(u32, String, String)>,
    ) -> Result<(), String> {
        not_yet("decorations2.patch-annotations")
    }
    pub(crate) fn start_session(
        &mut self,
        _argv: Vec<String>,
        _cwd: Option<String>,
        _stdin: Option<String>,
        _timeout_ms: Option<u32>,
    ) -> Result<u32, String> {
        not_yet("sessions.start")
    }
    pub(crate) fn session_write(&mut self, _id: u32, _data: String) -> Result<(), String> {
        not_yet("sessions.write")
    }
    pub(crate) fn session_close_input(&mut self, _id: u32) -> Result<(), String> {
        not_yet("sessions.close-input")
    }
    pub(crate) fn progress_begin(&mut self, _title: String) -> u32 {
        0
    }
    pub(crate) fn progress_report(&mut self, _token: u32, _message: String, _percent: Option<u8>) {}
    pub(crate) fn progress_end(&mut self, _token: u32) {}
    pub(crate) fn watch(&mut self, _path: String, _recursive: bool) -> Result<u32, String> {
        not_yet("watch.add")
    }
    pub(crate) fn lsp_servers(&mut self, _doc: DocumentId) -> Result<Vec<String>, String> {
        not_yet("lsp.servers")
    }
    pub(crate) fn lsp_request(
        &mut self,
        _doc: DocumentId,
        _server: Option<String>,
        _method: String,
        _params: String,
    ) -> Result<u32, String> {
        not_yet("lsp.request")
    }
}
