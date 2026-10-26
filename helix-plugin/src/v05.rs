//! API 0.5 on the host side: the types the editor sees, and the `State`
//! methods behind the 0.5 imports.

use crate::{DocumentId, Mode, State};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
    /// Not subscribable: a watch's changes go to the plugin that watches.
    FileChange,
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

/// Most key bindings per plugin.
pub const MAX_KEY_BINDINGS: usize = 256;

/// Checks a key sequence: `Err` says what is wrong with it.
pub type CheckKeys = fn(&str) -> Result<(), String>;

/// What the editor provides to API 0.5 calls
/// ([`crate::PluginHost::set_editor_fns`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct EditorFns {
    /// Check a key sequence ("space f", "C-k").
    pub check_keys: Option<CheckKeys>,
    /// Every native and typable command.
    pub commands: Option<fn() -> Vec<CommandEntry>>,
}

/// Most subscriptions per plugin.
pub const MAX_SUBSCRIPTIONS: usize = 64;

/// A plugin's API 0.5 state.
#[derive(Debug)]
pub(crate) struct V05 {
    /// The user's `allow-key-override`.
    pub key_override: bool,
    /// The user's `allow-lsp`.
    pub lsp: bool,
    pub editor: EditorFns,
    pub key_bindings: Vec<KeyBinding>,
    pub subscriptions: Vec<Subscription>,
    pub next_subscription: u32,
    /// Every loaded plugin's commands ("plugin/command", doc).
    pub plugin_commands: std::sync::Arc<Vec<(String, String)>>,
    /// Cleared when the plugin is disabled or unloaded.
    pub alive: Arc<AtomicBool>,
    /// The services this plugin registered.
    pub own_services: Vec<Service>,
    /// Every loaded plugin's services.
    pub services: Arc<Vec<Service>>,
    /// While serving a call: the plugins that led to it, first caller
    /// first.
    pub chain: Vec<String>,
    /// Every plugin's marks (the host's, shared).
    pub marks: SharedMarks,
    /// Running sessions' input: `None` once closed.
    pub sessions: HashMap<u32, Option<SessionWriter>>,
    /// Progress shown in the statusline: token -> title.
    pub progress: std::collections::BTreeMap<u32, String>,
    pub next_progress: u32,
}

/// Most progress reports a plugin shows at once.
pub const MAX_PROGRESS: usize = 8;

/// Most input a session may have waiting to be written.
pub const MAX_SESSION_QUEUE: usize = 1 << 20;

/// The plugin's end of a session's input.
#[derive(Debug)]
pub(crate) struct SessionWriter {
    sender: tokio::sync::mpsc::UnboundedSender<String>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
}

/// The editor's end of a session's input: what to write to the program,
/// in order, until the plugin closes it. Taken once.
#[derive(Debug, Clone)]
pub struct SessionInput {
    receiver: Arc<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<String>>>>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
}

impl SessionInput {
    /// The input and a counter of the bytes waiting: subtract what is
    /// written.
    pub fn take(
        &self,
    ) -> Option<(
        tokio::sync::mpsc::UnboundedReceiver<String>,
        Arc<std::sync::atomic::AtomicUsize>,
    )> {
        let receiver = self
            .receiver
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        Some((receiver, self.queued.clone()))
    }
}

impl Default for V05 {
    fn default() -> Self {
        Self {
            key_override: false,
            lsp: false,
            editor: EditorFns::default(),
            key_bindings: Vec::new(),
            subscriptions: Vec::new(),
            next_subscription: 0,
            plugin_commands: Default::default(),
            alive: Arc::new(AtomicBool::new(true)),
            own_services: Vec::new(),
            services: Default::default(),
            chain: Vec::new(),
            marks: Default::default(),
            sessions: HashMap::new(),
            progress: Default::default(),
            next_progress: 0,
        }
    }
}

/// Most marks per plugin.
pub const MAX_MARKS: usize = 10_000;

/// Positions plugins track in documents (`marks`), moved by every edit
/// ([`Marks::map`]).
#[derive(Debug, Default)]
pub struct Marks {
    next: u64,
    docs: HashMap<DocumentId, Vec<Mark>>,
    /// Each mark's document.
    index: HashMap<u64, DocumentId>,
    per_plugin: HashMap<String, usize>,
}

#[derive(Debug, Clone)]
struct Mark {
    id: u64,
    plugin: Arc<str>,
    pos: usize,
    gravity: Gravity,
}

pub type SharedMarks = Arc<std::sync::Mutex<Marks>>;

impl Marks {
    /// Follow an edit of `doc`. A mark goes when the text on both sides of
    /// it is deleted.
    pub fn map(&mut self, doc: DocumentId, changes: &helix_core::ChangeSet) {
        let Some(marks) = self.docs.get_mut(&doc) else {
            return;
        };
        let mut deleted = Vec::new();
        let mut pos = 0;
        for change in changes.changes() {
            match change {
                helix_core::Operation::Retain(n) => pos += n,
                helix_core::Operation::Delete(n) => {
                    deleted.push(pos..pos + n);
                    pos += n;
                }
                helix_core::Operation::Insert(_) => {}
            }
        }
        let mut gone = Vec::new();
        marks.retain_mut(|mark| {
            let i = deleted.partition_point(|range| range.end <= mark.pos);
            if deleted.get(i).is_some_and(|range| range.start < mark.pos) {
                gone.push((mark.id, mark.plugin.clone()));
                return false;
            }
            let assoc = match mark.gravity {
                Gravity::Before => helix_core::Assoc::Before,
                Gravity::After => helix_core::Assoc::After,
            };
            mark.pos = changes.map_pos(mark.pos, assoc);
            true
        });
        for (id, plugin) in gone {
            self.forget(id, &plugin);
        }
    }

    /// `doc` was closed: its marks go.
    pub fn close(&mut self, doc: DocumentId) {
        for mark in self.docs.remove(&doc).unwrap_or_default() {
            self.forget(mark.id, &mark.plugin);
        }
    }

    fn forget(&mut self, id: u64, plugin: &str) {
        self.index.remove(&id);
        if let Some(count) = self.per_plugin.get_mut(plugin) {
            *count = count.saturating_sub(1);
        }
    }

    /// Remove every mark of `plugin` (reloaded).
    pub(crate) fn remove_plugin(&mut self, plugin: &str) {
        for marks in self.docs.values_mut() {
            marks.retain(|mark| &*mark.plugin != plugin);
        }
        self.docs.retain(|_, marks| !marks.is_empty());
        self.index.retain(|_, doc| self.docs.contains_key(doc));
        let live: std::collections::HashSet<u64> = self
            .docs
            .values()
            .flat_map(|marks| marks.iter().map(|mark| mark.id))
            .collect();
        self.index.retain(|id, _| live.contains(id));
        self.per_plugin.remove(plugin);
    }

    pub(crate) fn set(
        &mut self,
        plugin: &str,
        doc: DocumentId,
        pos: usize,
        gravity: Gravity,
    ) -> Result<u64, String> {
        let count = self.per_plugin.entry(plugin.to_string()).or_default();
        if *count >= MAX_MARKS {
            return Err(format!("at most {MAX_MARKS} marks"));
        }
        *count += 1;
        self.next += 1;
        let id = self.next;
        self.docs.entry(doc).or_default().push(Mark {
            id,
            plugin: plugin.into(),
            pos,
            gravity,
        });
        self.index.insert(id, doc);
        Ok(id)
    }

    pub(crate) fn get(&self, plugin: &str, id: u64) -> Option<(DocumentId, usize)> {
        let doc = *self.index.get(&id)?;
        self.docs[&doc]
            .iter()
            .find(|mark| mark.id == id && &*mark.plugin == plugin)
            .map(|mark| (doc, mark.pos))
    }

    pub(crate) fn remove(&mut self, plugin: &str, id: u64) {
        let Some(&doc) = self.index.get(&id) else {
            return;
        };
        let marks = self.docs.get_mut(&doc).unwrap();
        let Some(i) = marks
            .iter()
            .position(|mark| mark.id == id && &*mark.plugin == plugin)
        else {
            return;
        };
        marks.swap_remove(i);
        if marks.is_empty() {
            self.docs.remove(&doc);
        }
        self.forget(id, plugin);
    }

    pub(crate) fn list(&self, plugin: &str, doc: DocumentId) -> Vec<(u64, u32)> {
        let mut marks: Vec<(u64, u32)> = self
            .docs
            .get(&doc)
            .into_iter()
            .flatten()
            .filter(|mark| &*mark.plugin == plugin)
            .map(|mark| (mark.id, mark.pos as u32))
            .collect();
        marks.sort_by_key(|&(id, pos)| (pos, id));
        marks
    }
}

/// Most services per plugin.
pub const MAX_SERVICES: usize = 32;
/// The longest chain of plugins calling each other's services.
pub const MAX_SERVICE_CHAIN: usize = 4;
/// Largest service payload or reply.
pub const MAX_SERVICE_PAYLOAD: usize = 1 << 20;

/// A registered service.
#[derive(Debug, Clone)]
pub(crate) struct Service {
    pub plugin: String,
    pub name: String,
    pub version: (u64, u64, u64),
    /// Plugins allowed to call (empty: any).
    pub callers: Vec<String>,
    pub alive: Arc<AtomicBool>,
}

impl Service {
    fn info(&self) -> ServiceInfo {
        let (major, minor, patch) = self.version;
        ServiceInfo {
            plugin: self.plugin.clone(),
            name: self.name.clone(),
            version: format!("{major}.{minor}.{patch}"),
        }
    }
    fn serves(&self, caller: &str) -> bool {
        self.alive.load(Ordering::Relaxed)
            && (self.callers.is_empty() || self.callers.iter().any(|c| c == caller))
    }
}

/// "major.minor.patch"
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// A call to a plugin's service, served after the caller's call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceCall {
    pub caller: String,
    pub id: u32,
    pub provider: String,
    pub service: String,
    pub method: String,
    pub payload: String,
    /// The plugins that led to this call, ending with the caller.
    pub chain: Vec<String>,
}

fn check_json(what: &str, json: &str) -> Result<(), String> {
    if json.len() > MAX_SERVICE_PAYLOAD {
        return Err(format!("{what} is larger than 1 MiB"));
    }
    serde_json::from_str::<serde::de::IgnoredAny>(json)
        .map(|_| ())
        .map_err(|err| format!("{what} is not JSON: {err}"))
}

/// Order plugins so dependencies load first. Plugins with a missing
/// required dependency (not in `plugins` and not `loaded`) or in a cycle
/// are left out, with an error each.
pub(crate) fn dependency_order<T>(
    plugins: Vec<(T, crate::Manifest, crate::PluginSettings)>,
    loaded: impl Fn(&str) -> bool,
) -> (
    Vec<(T, crate::Manifest, crate::PluginSettings)>,
    Vec<anyhow::Error>,
) {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        Visiting,
        Done,
        Failed,
    }
    let names: Vec<String> = plugins.iter().map(|(_, m, _)| m.name.clone()).collect();
    let index = |name: &str| names.iter().position(|n| n == name);
    let mut marks = vec![Mark::New; plugins.len()];
    let mut order = Vec::with_capacity(plugins.len());
    let mut errors = Vec::new();
    fn visit(
        i: usize,
        plugins: &[(impl Sized, crate::Manifest, crate::PluginSettings)],
        index: &dyn Fn(&str) -> Option<usize>,
        loaded: &dyn Fn(&str) -> bool,
        marks: &mut [Mark],
        order: &mut Vec<usize>,
        errors: &mut Vec<anyhow::Error>,
    ) -> bool {
        match marks[i] {
            Mark::Done => return true,
            Mark::Failed => return false,
            Mark::Visiting => {
                errors.push(anyhow::anyhow!(
                    "plugin '{}' is in a dependency cycle",
                    plugins[i].1.name
                ));
                marks[i] = Mark::Failed;
                return false;
            }
            Mark::New => {}
        }
        marks[i] = Mark::Visiting;
        let manifest = &plugins[i].1;
        let mut ok = true;
        for (dependency, required) in manifest
            .dependencies
            .iter()
            .map(|d| (d, true))
            .chain(manifest.optional_dependencies.iter().map(|d| (d, false)))
        {
            match index(dependency) {
                Some(j) => {
                    if !visit(j, plugins, index, loaded, marks, order, errors) && required {
                        if marks[i] != Mark::Failed {
                            errors.push(anyhow::anyhow!(
                                "plugin '{}' needs plugin '{dependency}', which can't load",
                                manifest.name
                            ));
                        }
                        ok = false;
                    }
                }
                None if required && !loaded(dependency) => {
                    errors.push(anyhow::anyhow!(
                        "plugin '{}' needs plugin '{dependency}', which isn't installed or enabled",
                        manifest.name
                    ));
                    ok = false;
                }
                None => {}
            }
        }
        if marks[i] == Mark::Failed {
            return false;
        }
        marks[i] = if ok { Mark::Done } else { Mark::Failed };
        if ok {
            order.push(i);
        }
        ok
    }
    for i in 0..plugins.len() {
        visit(
            i,
            &plugins,
            &index,
            &loaded,
            &mut marks,
            &mut order,
            &mut errors,
        );
    }
    let mut plugins: Vec<Option<_>> = plugins.into_iter().map(Some).collect();
    let ordered = order
        .into_iter()
        .filter_map(|i| plugins[i].take())
        .collect();
    (ordered, errors)
}

/// An event subscription with its filter (`subscriptions.subscribe`).
#[derive(Debug, Clone)]
pub(crate) struct Subscription {
    pub id: u32,
    pub event: SubscriptionEvent,
    pub filter: EventFilter,
    glob: Option<globset::GlobMatcher>,
}

impl Subscription {
    /// Whether an event about `doc` (and `command`) passes the filter. An
    /// event without a document passes document filters only if they are
    /// unset.
    pub fn matches(
        &self,
        doc: Option<(DocumentId, &crate::DocumentSnapshot)>,
        command: Option<&str>,
    ) -> bool {
        let filter = &self.filter;
        if let Some(command) = command {
            if !filter.commands.is_empty() && !filter.commands.iter().any(|c| c == command) {
                return false;
            }
        }
        let needs_doc =
            filter.document.is_some() || filter.language.is_some() || self.glob.is_some();
        let Some((id, doc)) = doc else {
            return !needs_doc;
        };
        filter.document.is_none_or(|wanted| wanted == id)
            && filter
                .language
                .as_deref()
                .is_none_or(|language| doc.language.as_deref() == Some(language))
            && self.glob.as_ref().is_none_or(|glob| {
                doc.path.as_deref().is_some_and(|path| {
                    glob.is_match(path) || path.file_name().is_some_and(|name| glob.is_match(name))
                })
            })
    }
}

impl SubscriptionEvent {
    /// The 0.1-0.4 event this is, if it is one.
    pub fn old_kind(self) -> Option<crate::EventKind> {
        use crate::EventKind as K;
        Some(match self {
            Self::DocumentOpen => K::DocumentOpen,
            Self::DocumentChange => K::DocumentChange,
            Self::DocumentSave => K::DocumentSave,
            Self::DocumentClose => K::DocumentClose,
            Self::SelectionChange => K::SelectionChange,
            Self::ModeSwitch => K::ModeSwitch,
            Self::PostCommand => K::PostCommand,
            Self::FocusGained => K::FocusGained,
            Self::FocusLost => K::FocusLost,
            Self::EditorIdle => K::EditorIdle,
            _ => return None,
        })
    }

    /// The subscription kind of an `events2` event.
    pub fn of(event: &Event2) -> Self {
        match event {
            Event2::BufferEnter(..) => Self::BufferEnter,
            Event2::BufferLeave(..) => Self::BufferLeave,
            Event2::ViewEnter(_) => Self::ViewEnter,
            Event2::ViewLeave(_) => Self::ViewLeave,
            Event2::CompletionMenu(_) => Self::CompletionMenu,
            Event2::ConfigChange(_) => Self::ConfigChange,
            Event2::PluginChange(..) => Self::PluginChange,
            Event2::FileChange(..) => Self::FileChange,
            Event2::Shutdown => Self::Shutdown,
        }
    }
}

/// `keys` with single spaces between keys.
fn normalize_keys(keys: &str) -> String {
    keys.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl State {
    pub(crate) fn set_key(&mut self, mut binding: KeyBinding) -> Result<(), String> {
        binding.keys = normalize_keys(&binding.keys);
        if binding.keys.is_empty() || binding.keys.len() > 100 {
            return Err("keys must be 1 to 100 characters".into());
        }
        if binding.name.is_empty() || binding.name.chars().count() > 64 {
            return Err("a binding's name must be 1 to 64 characters".into());
        }
        if binding.description.chars().count() > 200 {
            return Err("a binding's description must be at most 200 characters".into());
        }
        if binding.layer == KeyLayer::Override && !self.v05.key_override {
            return Err(format!(
                "the override layer needs `allow-key-override = true` under [{}] in plugins.toml",
                self.plugin
            ));
        }
        if let Some(check) = self.v05.editor.check_keys {
            check(&binding.keys)?;
        }
        let bindings = &mut self.v05.key_bindings;
        let same = |b: &KeyBinding| {
            b.mode == binding.mode && b.keys == binding.keys && b.document == binding.document
        };
        match bindings.iter().position(same) {
            Some(index) => bindings[index] = binding,
            None if bindings.len() >= MAX_KEY_BINDINGS => {
                return Err(format!(
                    "at most {MAX_KEY_BINDINGS} key bindings per plugin"
                ))
            }
            None => bindings.push(binding),
        }
        self.push_key_bindings();
        Ok(())
    }
    pub(crate) fn unset_key(
        &mut self,
        mode: Mode,
        keys: String,
        document: Option<DocumentId>,
    ) -> Result<(), String> {
        let keys = normalize_keys(&keys);
        let before = self.v05.key_bindings.len();
        self.v05
            .key_bindings
            .retain(|b| !(b.mode == mode && b.keys == keys && b.document == document));
        if self.v05.key_bindings.len() == before {
            return Err(format!("no binding of '{keys}'"));
        }
        self.push_key_bindings();
        Ok(())
    }
    pub(crate) fn list_keys(&mut self) -> Vec<KeyBinding> {
        self.v05.key_bindings.clone()
    }
    /// Tell the editor this plugin's bindings (all of them: the last action
    /// of a call wins).
    fn push_key_bindings(&mut self) {
        let plugin = self.plugin.clone();
        self.actions
            .retain(|action| !matches!(action, crate::Action::KeyBindings { plugin: p, .. } if *p == plugin));
        self.actions.push(crate::Action::KeyBindings {
            plugin,
            bindings: self.v05.key_bindings.clone(),
        });
    }
    pub(crate) fn menu_state(&mut self) -> Option<MenuState> {
        self.snapshot.completion_menu.clone()
    }
    pub(crate) fn menu_action(&mut self, action: MenuAction) -> Result<(), String> {
        self.need_ui("completion-menu")?;
        if let MenuAction::Select(Some(index)) = action {
            let count = self
                .snapshot
                .completion_menu
                .as_ref()
                .map_or(0, |menu| menu.items.len() as u32);
            if index >= count {
                return Err(format!("no completion item {index} (the menu has {count})"));
            }
        }
        self.actions.push(crate::Action::CompletionMenu(action));
        Ok(())
    }
    pub(crate) fn subscribe(
        &mut self,
        event: SubscriptionEvent,
        filter: EventFilter,
    ) -> Result<u32, String> {
        if event == SubscriptionEvent::PreCommand && filter.commands.is_empty() {
            return Err("pre-command needs the commands it is about (filter.commands)".into());
        }
        if filter.commands.len() > 256 {
            return Err("at most 256 commands in a filter".into());
        }
        if self.v05.subscriptions.len() >= MAX_SUBSCRIPTIONS {
            return Err(format!(
                "at most {MAX_SUBSCRIPTIONS} subscriptions per plugin"
            ));
        }
        let glob = match &filter.path_glob {
            Some(glob) => Some(
                globset::Glob::new(glob)
                    .map_err(|err| format!("invalid path glob '{glob}': {err}"))?
                    .compile_matcher(),
            ),
            None => None,
        };
        self.v05.next_subscription += 1;
        let id = self.v05.next_subscription;
        self.v05.subscriptions.push(Subscription {
            id,
            event,
            filter,
            glob,
        });
        self.subscriptions_changed();
        Ok(id)
    }
    pub(crate) fn unsubscribe(&mut self, id: u32) {
        let before = self.v05.subscriptions.len();
        self.v05.subscriptions.retain(|s| s.id != id);
        if self.v05.subscriptions.len() != before {
            self.subscriptions_changed();
        }
    }
    fn subscriptions_changed(&mut self) {
        if !self
            .actions
            .iter()
            .any(|action| matches!(action, crate::Action::SubscriptionsChanged))
        {
            self.actions.push(crate::Action::SubscriptionsChanged);
        }
    }
    pub(crate) fn command_list(&mut self) -> Vec<CommandEntry> {
        let mut commands = self
            .v05
            .editor
            .commands
            .map(|list| list())
            .unwrap_or_default();
        commands.extend(
            self.v05
                .plugin_commands
                .iter()
                .map(|(name, doc)| CommandEntry {
                    name: name.clone(),
                    doc: doc.clone(),
                    kind: CommandKind::Plugin,
                }),
        );
        commands
    }
    /// Whether `name` is a command (native, typable, or another plugin's).
    fn check_command(&self, name: &str) -> Result<(), String> {
        if name.contains('/') {
            return if self.v05.plugin_commands.iter().any(|(n, _)| n == name) {
                Ok(())
            } else {
                Err(format!("no plugin command '{name}'"))
            };
        }
        match self.v05.editor.commands {
            Some(list) if !list().iter().any(|command| command.name == name) => {
                Err(format!("no command '{name}'"))
            }
            _ => Ok(()),
        }
    }
    /// Whether this plugin may run command `name`: `run-commands`, and
    /// every program allowed for commands that start programs.
    fn check_may_run(&self, name: &str) -> Result<(), String> {
        self.need(crate::Permission::RunCommands)?;
        let starts = self
            .starts_program
            .is_some_and(|starts| starts(name) || starts(&format!(":{name}")));
        if starts && self.process != crate::ProcessGrant::Any {
            return Err(format!(
                "'{name}' runs programs: it needs allow-process = true in plugins.toml"
            ));
        }
        Ok(())
    }
    fn check_args(args: &[String]) -> Result<(), String> {
        if args.len() > 64 || args.iter().any(|arg| arg.len() > 64 << 10) {
            return Err("at most 64 arguments of at most 64 KiB".into());
        }
        Ok(())
    }
    pub(crate) fn execute(
        &mut self,
        name: String,
        args: Vec<String>,
        count: Option<u32>,
    ) -> Result<u32, String> {
        self.need_ui("commands.execute")?;
        self.check_command(&name)?;
        self.check_may_run(&name)?;
        Self::check_args(&args)?;
        let id = self.request(crate::PendingKind::Command)?;
        self.actions.push(crate::Action::Execute {
            plugin: self.plugin.clone(),
            id,
            name,
            args,
            count,
        });
        Ok(id)
    }
    pub(crate) fn batch(&mut self, steps: Vec<Step>) -> Result<u32, String> {
        self.need_ui("commands.batch")?;
        if steps.is_empty() || steps.len() > 100 {
            return Err("a batch has 1 to 100 steps".into());
        }
        for step in &steps {
            if let Step::Command { name, args, .. } = step {
                self.check_command(name)?;
                self.check_may_run(name)?;
                Self::check_args(args)?;
            }
        }
        let id = self.request(crate::PendingKind::Batch)?;
        self.actions.push(crate::Action::Batch {
            plugin: self.plugin.clone(),
            id,
            steps,
        });
        Ok(id)
    }
    pub(crate) fn set_mode(&mut self, mode: Mode) -> Result<(), String> {
        self.need_ui("commands.set-mode")?;
        self.actions.push(crate::Action::SetMode(mode));
        Ok(())
    }
    pub(crate) fn views(&mut self) -> Vec<ViewInfo> {
        self.snapshot.views.clone()
    }
    pub(crate) fn ui_state(&mut self) -> UiState {
        self.snapshot.ui.clone()
    }
    pub(crate) fn register_service(
        &mut self,
        name: String,
        version: String,
        callers: Vec<String>,
    ) -> Result<(), String> {
        if self.context != crate::CallContext::Init {
            return Err("services.register is only available from init".into());
        }
        if name.len() > 64 || !crate::is_valid_name(&name) {
            return Err(format!(
                "invalid service name '{name}': use a-z, 0-9, '-' and '_'"
            ));
        }
        let parsed = parse_version(&version)
            .ok_or_else(|| format!("invalid version '{version}': use major.minor.patch"))?;
        if callers.len() > 64 || callers.iter().any(|caller| !crate::is_valid_name(caller)) {
            return Err("callers: at most 64 plugin names".into());
        }
        if self
            .v05
            .own_services
            .iter()
            .any(|service| service.name == name)
        {
            return Err(format!("service '{name}' is already registered"));
        }
        if self.v05.own_services.len() >= MAX_SERVICES {
            return Err(format!("at most {MAX_SERVICES} services"));
        }
        self.v05.own_services.push(Service {
            plugin: self.plugin.clone(),
            name,
            version: parsed,
            callers,
            alive: self.v05.alive.clone(),
        });
        Ok(())
    }
    pub(crate) fn discover_services(&mut self, name: &str) -> Vec<ServiceInfo> {
        let (plugin, name) = match name.split_once('.') {
            Some((plugin, name)) => (Some(plugin), name),
            None => (None, name),
        };
        self.v05
            .services
            .iter()
            .filter(|service| {
                service.name == name
                    && plugin.is_none_or(|plugin| service.plugin == plugin)
                    && service.serves(&self.plugin)
            })
            .map(Service::info)
            .collect()
    }
    pub(crate) fn call_service(
        &mut self,
        service: String,
        version: String,
        method: String,
        payload: String,
    ) -> Result<u32, String> {
        if self.context == crate::CallContext::Init {
            return Err("services.call isn't available from init".into());
        }
        let (provider, name) = service
            .split_once('.')
            .ok_or_else(|| format!("'{service}': name a service as plugin.name"))?;
        let want = parse_version(&version)
            .ok_or_else(|| format!("invalid version '{version}': use major.minor.patch"))?;
        if method.is_empty() || method.len() > 256 {
            return Err("a method of 1 to 256 bytes".into());
        }
        check_json("the payload", &payload)?;
        let found = self
            .v05
            .services
            .iter()
            .find(|s| s.plugin == provider && s.name == name)
            .ok_or_else(|| format!("no service '{service}'"))?;
        if !found.alive.load(Ordering::Relaxed) {
            return Err(format!("plugin '{provider}' is unavailable"));
        }
        if !found.serves(&self.plugin) {
            return Err(format!(
                "'{service}' doesn't accept calls from '{}'",
                self.plugin
            ));
        }
        let have = found.version;
        if have.0 != want.0 || (have.1, have.2) < (want.1, want.2) {
            return Err(format!(
                "'{service}' is version {}.{}.{}, not compatible with {version}",
                have.0, have.1, have.2
            ));
        }
        let mut chain = self.v05.chain.clone();
        chain.push(self.plugin.clone());
        if chain.iter().any(|plugin| plugin == provider) {
            return Err(format!(
                "service call cycle: {} -> {provider}",
                chain.join(" -> ")
            ));
        }
        if chain.len() > MAX_SERVICE_CHAIN {
            return Err(format!(
                "service calls nest at most {MAX_SERVICE_CHAIN} plugins deep"
            ));
        }
        let id = self.request(crate::PendingKind::Service)?;
        self.actions.push(crate::Action::ServiceCall(ServiceCall {
            caller: self.plugin.clone(),
            id,
            provider: provider.to_string(),
            service: name.to_string(),
            method,
            payload,
            chain,
        }));
        Ok(id)
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
    fn marks(&self) -> std::sync::MutexGuard<'_, Marks> {
        self.v05.marks.lock().unwrap_or_else(|err| err.into_inner())
    }
    pub(crate) fn set_mark(
        &mut self,
        doc: DocumentId,
        position: u32,
        gravity: Gravity,
    ) -> Result<u64, String> {
        let len = self.document(doc)?.text.len_chars();
        if position as usize > len {
            return Err(format!(
                "invalid position {position} for a document of {len} chars"
            ));
        }
        let plugin = self.plugin.clone();
        self.marks().set(&plugin, doc, position as usize, gravity)
    }
    pub(crate) fn get_mark(&mut self, id: u64) -> Option<(DocumentId, u32)> {
        self.marks()
            .get(&self.plugin, id)
            .map(|(doc, pos)| (doc, pos as u32))
    }
    pub(crate) fn remove_mark(&mut self, id: u64) {
        let plugin = self.plugin.clone();
        self.marks().remove(&plugin, id);
    }
    pub(crate) fn list_marks(&mut self, doc: DocumentId) -> Vec<(u64, u32)> {
        self.marks().list(&self.plugin, doc)
    }
    pub(crate) fn set_virtual_lines(
        &mut self,
        doc: DocumentId,
        layer: String,
        lines: Vec<crate::PluginVirtualLine>,
    ) -> Result<(), String> {
        self.need_layer(doc, &layer, lines.len())?;
        let text = self.document(doc)?.text.slice(..);
        let mut checked = Vec::with_capacity(lines.len());
        for line in lines {
            if line.line >= text.len_lines() {
                return Err(format!(
                    "invalid line {} for a document of {} lines",
                    line.line,
                    text.len_lines()
                ));
            }
            if line.text.contains(['\n', '\r']) {
                return Err("a virtual line can't contain line breaks".into());
            }
            let line_break = helix_core::line_ending::line_end_char_index(&text, line.line);
            checked.push((line_break, line.text, line.scope));
        }
        self.decorate(doc, layer, crate::Decorations::VirtualLines(checked));
        Ok(())
    }
    /// `from..to` within a document of `len` chars.
    fn patch_range(from: u32, to: u32, len: usize) -> Result<std::ops::Range<usize>, String> {
        let (from, to) = (from as usize, to as usize);
        if from > to || to > len {
            return Err(format!(
                "invalid range {from}..{to} for a document of {len} chars"
            ));
        }
        Ok(from..to)
    }
    pub(crate) fn patch_highlights(
        &mut self,
        doc: DocumentId,
        layer: String,
        from: u32,
        to: u32,
        highlights: Vec<(u32, u32, String)>,
    ) -> Result<(), String> {
        let len = self.need_layer(doc, &layer, highlights.len())?;
        let range = Self::patch_range(from, to, len)?;
        let mut checked = Vec::with_capacity(highlights.len());
        for (start, end, scope) in highlights {
            let (start, end) = (start as usize, end as usize);
            if !range.contains(&start) || end < start || end > len {
                return Err(format!(
                    "highlight {start}..{end} doesn't start in {}..{}",
                    range.start, range.end
                ));
            }
            checked.push((start..end, scope));
        }
        self.decorate(
            doc,
            layer,
            crate::Decorations::PatchHighlights(range, checked),
        );
        Ok(())
    }
    pub(crate) fn patch_annotations(
        &mut self,
        doc: DocumentId,
        layer: String,
        from: u32,
        to: u32,
        annotations: Vec<(u32, String, String)>,
    ) -> Result<(), String> {
        let len = self.need_layer(doc, &layer, annotations.len())?;
        let range = Self::patch_range(from, to, len)?;
        let mut checked = Vec::with_capacity(annotations.len());
        for (position, text, scope) in annotations {
            let position = position as usize;
            if !(range.contains(&position) || (position == range.end && position == len)) {
                return Err(format!(
                    "position {position} isn't in {}..{}",
                    range.start, range.end
                ));
            }
            if text.contains('\n') {
                return Err("virtual text can't contain line breaks".into());
            }
            checked.push((position, text, scope));
        }
        self.decorate(
            doc,
            layer,
            crate::Decorations::PatchAnnotations(range, checked),
        );
        Ok(())
    }
    pub(crate) fn start_session(
        &mut self,
        argv: Vec<String>,
        cwd: Option<String>,
        stdin: Option<String>,
        timeout_ms: Option<u32>,
    ) -> Result<u32, String> {
        self.need_process(&argv)?;
        let timeout = timeout_ms
            .map(|ms| std::time::Duration::from_millis(ms.into()))
            .unwrap_or(crate::DEFAULT_PROCESS_TIMEOUT)
            .min(crate::MAX_SESSION_TIMEOUT);
        let id = self.request(crate::PendingKind::Process)?;
        self.streaming.insert(id);
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let queued = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Ended sessions' writers go when a new one starts.
        let pending = &self.pending;
        self.v05.sessions.retain(|id, _| pending.contains_key(id));
        self.v05.sessions.insert(
            id,
            Some(SessionWriter {
                sender,
                queued: queued.clone(),
            }),
        );
        self.actions.push(crate::Action::Spawn {
            plugin: self.plugin.clone(),
            id,
            argv,
            cwd: cwd.map(std::path::PathBuf::from),
            stdin,
            timeout,
            stream: true,
            session: Some(SessionInput {
                receiver: Arc::new(std::sync::Mutex::new(Some(receiver))),
                queued,
            }),
        });
        Ok(id)
    }
    fn session(&mut self, id: u32) -> Result<&mut Option<SessionWriter>, String> {
        if self.pending.get(&id) != Some(&crate::PendingKind::Process) {
            self.v05.sessions.remove(&id);
        }
        self.v05
            .sessions
            .get_mut(&id)
            .ok_or_else(|| format!("no running session {id}"))
    }
    pub(crate) fn session_write(&mut self, id: u32, data: String) -> Result<(), String> {
        let writer = self
            .session(id)?
            .as_ref()
            .ok_or_else(|| format!("session {id}'s input is closed"))?;
        let queued = writer.queued.load(Ordering::Relaxed);
        if data.len() > MAX_SESSION_QUEUE {
            return Err("at most 1 MiB per write".into());
        }
        if queued + data.len() > MAX_SESSION_QUEUE {
            return Err("busy".into());
        }
        writer.queued.fetch_add(data.len(), Ordering::Relaxed);
        // The program ended: its result is on the way.
        let _ = writer.sender.send(data);
        Ok(())
    }
    pub(crate) fn session_close_input(&mut self, id: u32) -> Result<(), String> {
        // Dropping the sender closes the program's input once the rest is
        // written.
        self.session(id)?.take();
        Ok(())
    }
    /// Progress is a statusline segment ("progress-<token>").
    fn show_progress(&mut self, token: u32, text: String) {
        let _ = self.set_statusline(format!("progress-{token}"), text);
    }
    pub(crate) fn progress_begin(&mut self, title: String) -> u32 {
        // The oldest goes to make room.
        if self.v05.progress.len() >= MAX_PROGRESS {
            if let Some((&oldest, _)) = self.v05.progress.iter().next() {
                self.progress_end(oldest);
            }
        }
        self.v05.next_progress = self.v05.next_progress.wrapping_add(1).max(1);
        let token = self.v05.next_progress;
        let title: String = title.chars().take(64).collect();
        self.show_progress(token, format!("{title}…"));
        self.v05.progress.insert(token, title);
        token
    }
    pub(crate) fn progress_report(&mut self, token: u32, message: String, percent: Option<u8>) {
        let Some(title) = self.v05.progress.get(&token) else {
            return;
        };
        let mut text = title.clone();
        if !message.is_empty() {
            text.push_str(": ");
            text.extend(message.chars().take(80));
        }
        if let Some(percent) = percent {
            text.push_str(&format!(" {}%", percent.min(100)));
        }
        self.show_progress(token, text);
    }
    pub(crate) fn progress_end(&mut self, token: u32) {
        if self.v05.progress.remove(&token).is_some() {
            self.show_progress(token, String::new());
        }
    }
    pub(crate) fn watch(&mut self, path: String, recursive: bool) -> Result<u32, String> {
        self.need(crate::Permission::Filesystem)?;
        if self.files == crate::FileGrant::None {
            return Err("watching needs allow-files in plugins.toml".into());
        }
        if self.context == crate::CallContext::Init {
            return Err("watch.add can't be used from init".into());
        }
        let path = std::path::PathBuf::from(path);
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
        // Inside a root, also through symlinks.
        let real = std::fs::canonicalize(&path)
            .map_err(|err| format!("can't watch {}: {err}", path.display()))?;
        let inside = self.roots.iter().any(|root| {
            path.starts_with(root)
                && std::fs::canonicalize(root).is_ok_and(|root| real.starts_with(root))
        });
        if !inside {
            return Err(format!(
                "{} is outside the directories this plugin may read",
                path.display()
            ));
        }
        let watching = self
            .pending
            .values()
            .filter(|kind| **kind == crate::PendingKind::Watch)
            .count();
        if watching >= MAX_WATCHES {
            return Err(format!("at most {MAX_WATCHES} watched paths"));
        }
        let id = self.request(crate::PendingKind::Watch)?;
        self.actions.push(crate::Action::Watch {
            plugin: self.plugin.clone(),
            id,
            path: real,
            recursive,
        });
        Ok(id)
    }
    fn need_lsp(&self) -> Result<(), String> {
        self.need(crate::Permission::Lsp)?;
        if !self.v05.lsp {
            return Err(format!(
                "language server requests need allow-lsp = true under [{}] in plugins.toml",
                self.plugin
            ));
        }
        Ok(())
    }
    pub(crate) fn lsp_servers(&mut self, doc: DocumentId) -> Result<Vec<String>, String> {
        self.need_lsp()?;
        Ok(self.document(doc)?.language_servers.clone())
    }
    pub(crate) fn lsp_request(
        &mut self,
        doc: DocumentId,
        server: Option<String>,
        method: String,
        params: String,
    ) -> Result<u32, String> {
        self.need_lsp()?;
        if self.context == crate::CallContext::Init {
            return Err("lsp.request can't be used from init".into());
        }
        let servers = &self.document(doc)?.language_servers;
        match &server {
            Some(name) if !servers.contains(name) => {
                return Err(format!("no language server '{name}' on this document"))
            }
            None if servers.is_empty() => return Err("no language server on this document".into()),
            _ => {}
        }
        if LSP_LIFECYCLE.contains(&method.as_str()) || method.starts_with("$/") {
            return Err(format!("'{method}' is the editor's to send"));
        }
        if method.is_empty() || method.len() > 256 {
            return Err("a method of 1 to 256 bytes".into());
        }
        check_json("the params", &params)?;
        let id = self.request(crate::PendingKind::Lsp)?;
        self.actions.push(crate::Action::LspRequest {
            plugin: self.plugin.clone(),
            id,
            doc,
            server,
            method,
            params,
        });
        Ok(id)
    }
}

/// Most watched paths per plugin.
pub const MAX_WATCHES: usize = 16;

/// Requests that would break the editor's session with a server.
const LSP_LIFECYCLE: &[&str] = &["initialize", "shutdown", "exit", "initialized"];

impl crate::PluginHost {
    /// Paths under watch `id` of `plugin` changed: tell it (if it still
    /// watches).
    pub fn file_change(
        &mut self,
        plugin: &str,
        id: u32,
        paths: Vec<String>,
        snapshot: &crate::Snapshot,
    ) -> Option<anyhow::Result<Vec<crate::Action>>> {
        let watching = self.plugins.iter().any(|p| {
            p.manifest.name == plugin
                && p.store.data().pending.get(&id) == Some(&crate::PendingKind::Watch)
        });
        if !watching {
            return None;
        }
        self.dispatch2_to(plugin, &Event2::FileChange(id, paths), snapshot)
    }
}

impl crate::Event {
    /// The document the event is about.
    fn document(&self) -> Option<DocumentId> {
        use crate::Event as E;
        match self {
            E::DocumentOpen(doc)
            | E::DocumentChange(doc)
            | E::DocumentSave(doc)
            | E::DocumentClose(doc)
            | E::SelectionChange(doc)
            | E::BufferWrite(doc) => Some(*doc),
            E::Completion { doc, .. } => Some(*doc),
            _ => None,
        }
    }
}

/// `doc` with its snapshot, if the snapshot has it.
fn with_snapshot(
    doc: Option<DocumentId>,
    snapshot: &crate::Snapshot,
) -> Option<(DocumentId, &crate::DocumentSnapshot)> {
    let doc = doc?;
    Some((doc, snapshot.documents.get(&doc)?))
}

impl crate::Plugin {
    fn subscriptions(&self) -> &[Subscription] {
        &self.store.data().v05.subscriptions
    }

    /// Subscribed to events of `kind` at all (in `init` or later).
    pub(crate) fn subscribed(&self, kind: crate::EventKind) -> bool {
        self.events.contains(&kind)
            || self
                .subscriptions()
                .iter()
                .any(|s| s.event.old_kind() == Some(kind))
    }

    /// Whether `event` goes to this plugin: subscribed in `init`, or a
    /// subscription's filter lets it through.
    pub(crate) fn wants_event(&self, event: &crate::Event, snapshot: &crate::Snapshot) -> bool {
        let kind = event.kind();
        if self.events.contains(&kind) {
            return true;
        }
        let doc = with_snapshot(event.document(), snapshot);
        let command = match event {
            crate::Event::PostCommand(command) => Some(command.as_str()),
            _ => None,
        };
        self.subscriptions()
            .iter()
            .any(|s| s.event.old_kind() == Some(kind) && s.matches(doc, command))
    }

    /// Whether an `events2` event or hook of `kind` about `doc` (and
    /// `command`) goes to this plugin.
    fn wants2(
        &self,
        kind: SubscriptionEvent,
        doc: Option<(DocumentId, &crate::DocumentSnapshot)>,
        command: Option<&str>,
    ) -> bool {
        self.disabled.is_none()
            && self.manifest.at_least_0_5()
            && self
                .subscriptions()
                .iter()
                .any(|s| s.event == kind && s.matches(doc, command))
    }
}

/// What a hook decided, and the actions of the plugins asked.
#[derive(Debug, Default)]
pub struct HookResult {
    /// `(plugin, reason)` of the plugin that cancelled.
    pub cancelled: Option<(String, String)>,
    /// Each asked plugin's actions (or error), in the order asked.
    pub results: Vec<(String, anyhow::Result<Vec<crate::Action>>)>,
}

impl crate::PluginHost {
    /// Whether an enabled API 0.5 plugin subscribed to `kind` (an `events2`
    /// event or a hook), whatever the filter.
    pub fn wants2(&self, kind: SubscriptionEvent) -> bool {
        self.plugins.iter().any(|plugin| {
            plugin.disabled.is_none()
                && plugin.manifest.at_least_0_5()
                && plugin.subscriptions().iter().any(|s| s.event == kind)
        })
    }

    /// Send `event` (about `doc`) to the plugins whose subscriptions let it
    /// through, in load order.
    pub fn dispatch2(
        &mut self,
        event: &Event2,
        doc: Option<DocumentId>,
        snapshot: &crate::Snapshot,
    ) -> Vec<(String, anyhow::Result<Vec<crate::Action>>)> {
        let kind = SubscriptionEvent::of(event);
        let names: Vec<String> = self
            .plugins
            .iter()
            .filter(|plugin| plugin.wants2(kind, with_snapshot(doc, snapshot), None))
            .map(|plugin| plugin.manifest.name.clone())
            .collect();
        names
            .into_iter()
            .filter_map(|name| {
                let result = self.dispatch2_to(&name, event, snapshot)?;
                Some((name, result))
            })
            .collect()
    }

    /// Send `event` to `plugin` (API 0.5, enabled) whether or not it
    /// subscribed: a watch's changes, its own settings.
    pub fn dispatch2_to(
        &mut self,
        plugin: &str,
        event: &Event2,
        snapshot: &crate::Snapshot,
    ) -> Option<anyhow::Result<Vec<crate::Action>>> {
        let limit = self.limits.event;
        let plugin = self.plugins.iter_mut().find(|p| {
            p.manifest.name == plugin && p.disabled.is_none() && p.manifest.at_least_0_5()
        })?;
        let event = event.clone();
        Some(
            plugin
                .call(
                    snapshot.clone(),
                    crate::CallContext::Event,
                    limit,
                    move |guest, store| {
                        Box::pin(async move { guest.call_event2(store, &event).await })
                    },
                )
                .map(|((), actions)| actions),
        )
    }

    /// Ask the plugins that subscribed to `pre-command` for `command`, in
    /// load order, until one cancels it.
    pub fn pre_command(&mut self, command: &str, snapshot: &crate::Snapshot) -> HookResult {
        let names: Vec<String> = self
            .plugins
            .iter()
            .filter(|p| p.wants2(SubscriptionEvent::PreCommand, None, Some(command)))
            .map(|p| p.manifest.name.clone())
            .collect();
        let command = command.to_string();
        self.run_hook(names, snapshot, move |guest, store| {
            let command = command.clone();
            Box::pin(async move { guest.call_pre_command(store, &command).await })
        })
    }

    /// Ask the plugins that subscribed to `pre-save` (with a filter `doc`
    /// passes) before writing `doc`, until one cancels. Their edits are
    /// applied before writing.
    pub fn pre_save(&mut self, doc: DocumentId, snapshot: &crate::Snapshot) -> HookResult {
        let names: Vec<String> = self
            .plugins
            .iter()
            .filter(|p| {
                p.wants2(
                    SubscriptionEvent::PreSave,
                    with_snapshot(Some(doc), snapshot),
                    None,
                )
            })
            .map(|p| p.manifest.name.clone())
            .collect();
        self.run_hook(names, snapshot, move |guest, store| {
            Box::pin(async move { guest.call_pre_save(store, doc).await })
        })
    }

    fn run_hook(
        &mut self,
        names: Vec<String>,
        snapshot: &crate::Snapshot,
        call: impl for<'a> Fn(
                &'a crate::api::Instance,
                &'a mut wasmtime::Store<State>,
            ) -> crate::CallFuture<'a, HookOutcome>
            + Send
            + Clone,
    ) -> HookResult {
        let limit = self.limits.hook;
        let mut result = HookResult::default();
        for name in names {
            let Some(plugin) = self
                .plugins
                .iter_mut()
                .find(|p| p.manifest.name == name && p.disabled.is_none())
            else {
                continue;
            };
            let call = call.clone();
            match plugin.call(
                snapshot.clone(),
                crate::CallContext::Event,
                limit,
                move |guest, store| call(guest, store),
            ) {
                Ok((outcome, actions)) => {
                    result.results.push((name.clone(), Ok(actions)));
                    if let HookOutcome::Cancel(reason) = outcome {
                        result.cancelled = Some((name, reason));
                        break;
                    }
                }
                // A failing plugin is disabled and counts as `proceed`.
                Err(err) => result.results.push((name, Err(err))),
            }
        }
        result
    }
}

impl crate::PluginHost {
    /// Give every plugin the list of all plugins' commands and services
    /// (after loading).
    pub(crate) fn refresh_shared(&mut self) {
        self.refresh_command_lists();
        let services: Vec<Service> = self
            .plugins
            .iter()
            .flat_map(|plugin| plugin.store.data().v05.own_services.iter().cloned())
            .collect();
        let services = Arc::new(services);
        for plugin in &mut self.plugins {
            plugin.store.data_mut().v05.services = services.clone();
        }
    }

    /// Serve a service call: run the provider's `on-service-call` (with
    /// its own permissions, told who called) and return the reply and what
    /// the provider does.
    pub fn serve(
        &mut self,
        call: &ServiceCall,
        snapshot: &crate::Snapshot,
    ) -> (Result<String, String>, Vec<crate::Action>) {
        let limit = self.limits.event;
        let Some(plugin) = self.plugins.iter_mut().find(|p| {
            p.manifest.name == call.provider
                && p.disabled.is_none()
                && p.store
                    .data()
                    .v05
                    .own_services
                    .iter()
                    .any(|s| s.name == call.service)
        }) else {
            return (
                Err(format!("plugin '{}' is unavailable", call.provider)),
                Vec::new(),
            );
        };
        plugin.store.data_mut().v05.chain = call.chain.clone();
        let (caller, service, method, payload) = (
            call.caller.clone(),
            format!("{}.{}", call.provider, call.service),
            call.method.clone(),
            call.payload.clone(),
        );
        let result = plugin.call(
            snapshot.clone(),
            crate::CallContext::Event,
            limit,
            move |guest, store| {
                Box::pin(async move {
                    guest
                        .call_service(store, &caller, &service, &method, &payload)
                        .await
                })
            },
        );
        plugin.store.data_mut().v05.chain.clear();
        match result {
            Ok((Ok(reply), actions)) => match check_json("the reply", &reply) {
                Ok(()) => (Ok(reply), actions),
                Err(err) => (Err(format!("'{}': {err}", call.provider)), actions),
            },
            Ok((Err(err), actions)) => (Err(err), actions),
            Err(err) => (Err(format!("{err:#}")), Vec::new()),
        }
    }

    /// Give every plugin the list of all plugins' commands (after loading).
    pub(crate) fn refresh_command_lists(&mut self) {
        let commands: Vec<(String, String)> = self
            .commands()
            .into_iter()
            .map(|(plugin, command)| (format!("{plugin}/{}", command.name), command.doc))
            .collect();
        let commands = std::sync::Arc::new(commands);
        for plugin in &mut self.plugins {
            plugin.store.data_mut().v05.plugin_commands = commands.clone();
        }
    }

    /// Deliver an API 0.5 result to `plugin` (if it still waits for it) and
    /// return what it does.
    pub fn callback2(
        &mut self,
        plugin: &str,
        callback: Callback2,
        snapshot: &crate::Snapshot,
    ) -> anyhow::Result<Vec<crate::Action>> {
        let limit = self.limits.command;
        let Some(plugin) = self
            .plugins
            .iter_mut()
            .find(|p| p.manifest.name == plugin && p.disabled.is_none())
        else {
            return Ok(Vec::new());
        };
        let (id, kind) = match &callback {
            Callback2::CommandDone { id, .. } => (*id, crate::PendingKind::Command),
            Callback2::BatchDone { id, .. } => (*id, crate::PendingKind::Batch),
            Callback2::ServiceReply { id, .. } => (*id, crate::PendingKind::Service),
            Callback2::LspReply { id, .. } => (*id, crate::PendingKind::Lsp),
        };
        if plugin.store.data().pending.get(&id) != Some(&kind) {
            return Ok(Vec::new());
        }
        plugin.store.data_mut().pending.remove(&id);
        plugin
            .call(
                snapshot.clone(),
                crate::CallContext::Callback,
                limit,
                move |guest, store| {
                    Box::pin(async move { guest.call_callback2(store, &callback).await })
                },
            )
            .map(|((), actions)| actions)
    }
}
