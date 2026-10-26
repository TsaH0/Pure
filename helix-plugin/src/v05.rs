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

fn not_yet<T>(what: &str) -> Result<T, String> {
    Err(format!("{what} is not available yet"))
}

/// Most key bindings per plugin.
pub const MAX_KEY_BINDINGS: usize = 256;

/// What the editor provides to API 0.5 calls
/// ([`crate::PluginHost::set_editor_fns`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct EditorFns {
    /// Check a key sequence ("space f", "C-k").
    pub check_keys: Option<fn(&str) -> Result<(), String>>,
    /// Every native, typable and plugin command.
    pub commands: Option<fn() -> Vec<CommandEntry>>,
}

/// Most subscriptions per plugin.
pub const MAX_SUBSCRIPTIONS: usize = 64;

/// A plugin's API 0.5 state.
#[derive(Debug, Default)]
pub(crate) struct V05 {
    /// The user's `allow-key-override`.
    pub key_override: bool,
    pub editor: EditorFns,
    pub key_bindings: Vec<KeyBinding>,
    pub subscriptions: Vec<Subscription>,
    pub next_subscription: u32,
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
