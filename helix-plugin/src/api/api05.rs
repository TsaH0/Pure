//! API 0.5 adapters: 0.4's, plus key callbacks, the completion menu,
//! subscriptions, commands, state, services, floats, marks, more
//! decorations, sessions, progress, watches, language servers, and the
//! hooks, events and callbacks exports.
use super::api04::v04_adapters;
use super::v0_5;
use super::world::world_adapters;
use crate::v05::{
    self, CloseReason, CommandKind, CompletionEvent, Event2, FloatPosition, Gravity, HookOutcome,
    KeyLayer, KeyOutcome, MenuAction, PluginState, Step, SubscriptionEvent,
};
use v0_5::helix::plugin::types::Mode as M5;

world_adapters!(v0_5, files_done);
v04_adapters!(v0_5);

fn mode(mode: M5) -> crate::Mode {
    match mode {
        M5::Normal => crate::Mode::Normal,
        M5::Insert => crate::Mode::Insert,
        M5::Select => crate::Mode::Select,
    }
}

fn to_mode(mode: crate::Mode) -> M5 {
    match mode {
        crate::Mode::Normal => M5::Normal,
        crate::Mode::Insert => M5::Insert,
        crate::Mode::Select => M5::Select,
    }
}

mod keymaps_impl {
    use super::*;
    use v0_5::helix::plugin::keymaps::{Binding, Host, Layer};

    fn layer(layer: Layer) -> KeyLayer {
        match layer {
            Layer::Override => KeyLayer::Override,
            Layer::Plugin => KeyLayer::Plugin,
            Layer::Fallback => KeyLayer::Fallback,
        }
    }

    fn to_layer(layer: KeyLayer) -> Layer {
        match layer {
            KeyLayer::Override => Layer::Override,
            KeyLayer::Plugin => Layer::Plugin,
            KeyLayer::Fallback => Layer::Fallback,
        }
    }

    impl Host for State {
        fn set(&mut self, binding: Binding) -> std::result::Result<(), String> {
            State::set_key(
                self,
                v05::KeyBinding {
                    mode: mode(binding.mode),
                    keys: binding.keys,
                    name: binding.name,
                    description: binding.description,
                    layer: layer(binding.layer),
                    document: binding.document,
                    language: binding.language,
                },
            )
        }
        fn unset(
            &mut self,
            at: M5,
            keys: String,
            document: Option<u32>,
        ) -> std::result::Result<(), String> {
            State::unset_key(self, mode(at), keys, document)
        }
        fn list(&mut self) -> Vec<Binding> {
            State::list_keys(self)
                .into_iter()
                .map(|binding| Binding {
                    mode: to_mode(binding.mode),
                    keys: binding.keys,
                    name: binding.name,
                    description: binding.description,
                    layer: to_layer(binding.layer),
                    document: binding.document,
                    language: binding.language,
                })
                .collect()
        }
    }
}

mod completion_menu_impl {
    use super::*;
    use v0_5::helix::plugin::completion_menu::{Host, MenuState};

    impl Host for State {
        fn state(&mut self) -> Option<MenuState> {
            State::menu_state(self).map(|menu| MenuState {
                items: menu.items,
                selected: menu.selected,
                previewing: menu.previewing,
            })
        }
        fn select(&mut self, index: Option<u32>) -> std::result::Result<(), String> {
            State::menu_action(self, MenuAction::Select(index))
        }
        fn next(&mut self) -> std::result::Result<(), String> {
            State::menu_action(self, MenuAction::Next)
        }
        fn previous(&mut self) -> std::result::Result<(), String> {
            State::menu_action(self, MenuAction::Previous)
        }
        fn confirm(&mut self) -> std::result::Result<(), String> {
            State::menu_action(self, MenuAction::Confirm)
        }
        fn abort(&mut self) -> std::result::Result<(), String> {
            State::menu_action(self, MenuAction::Abort)
        }
        fn close(&mut self) -> std::result::Result<(), String> {
            State::menu_action(self, MenuAction::Close)
        }
    }
}

mod subscriptions_impl {
    use super::*;
    use v0_5::helix::plugin::subscriptions::{EventType, Filter, Host};

    fn event(event: EventType) -> SubscriptionEvent {
        use SubscriptionEvent as S;
        match event {
            EventType::DocumentOpen => S::DocumentOpen,
            EventType::DocumentChange => S::DocumentChange,
            EventType::DocumentSave => S::DocumentSave,
            EventType::DocumentClose => S::DocumentClose,
            EventType::SelectionChange => S::SelectionChange,
            EventType::ModeSwitch => S::ModeSwitch,
            EventType::PostCommand => S::PostCommand,
            EventType::FocusGained => S::FocusGained,
            EventType::FocusLost => S::FocusLost,
            EventType::EditorIdle => S::EditorIdle,
            EventType::BufferEnter => S::BufferEnter,
            EventType::BufferLeave => S::BufferLeave,
            EventType::ViewEnter => S::ViewEnter,
            EventType::ViewLeave => S::ViewLeave,
            EventType::CompletionMenu => S::CompletionMenu,
            EventType::ConfigChange => S::ConfigChange,
            EventType::PluginChange => S::PluginChange,
            EventType::Shutdown => S::Shutdown,
            EventType::PreCommand => S::PreCommand,
            EventType::PreSave => S::PreSave,
        }
    }

    impl Host for State {
        fn subscribe(
            &mut self,
            kind: EventType,
            filter: Filter,
        ) -> std::result::Result<u32, String> {
            State::subscribe(
                self,
                event(kind),
                v05::EventFilter {
                    language: filter.language,
                    path_glob: filter.path_glob,
                    document: filter.document,
                    commands: filter.commands,
                },
            )
        }
        fn unsubscribe(&mut self, id: u32) {
            State::unsubscribe(self, id)
        }
    }
}

mod commands_impl {
    use super::*;
    use v0_5::helix::plugin::commands::{Command, CommandKind as K, Host, Step as S};

    impl Host for State {
        fn list(&mut self) -> Vec<Command> {
            State::command_list(self)
                .into_iter()
                .map(|command| Command {
                    name: command.name,
                    doc: command.doc,
                    kind: match command.kind {
                        CommandKind::Native => K::Native,
                        CommandKind::Typable => K::Typable,
                        CommandKind::Plugin => K::Plugin,
                    },
                })
                .collect()
        }
        fn execute(
            &mut self,
            name: String,
            args: Vec<String>,
            count: Option<u32>,
        ) -> std::result::Result<u32, String> {
            State::execute(self, name, args, count)
        }
        fn batch(&mut self, steps: Vec<S>) -> std::result::Result<u32, String> {
            let steps = steps
                .into_iter()
                .map(|step| match step {
                    S::Command((name, args, count)) => Step::Command { name, args, count },
                    S::SetMode(m) => Step::SetMode(mode(m)),
                })
                .collect();
            State::batch(self, steps)
        }
        fn set_mode(&mut self, m: M5) -> std::result::Result<(), String> {
            State::set_mode(self, mode(m))
        }
    }
}

mod state_impl {
    use super::*;
    use v0_5::helix::plugin::state::{Area, Host, UiState, ViewInfo};

    impl Host for State {
        fn views(&mut self) -> Vec<ViewInfo> {
            State::views(self)
                .into_iter()
                .map(|view| ViewInfo {
                    id: view.id,
                    document: view.document,
                    area: Area {
                        x: view.area.0,
                        y: view.area.1,
                        width: view.area.2,
                        height: view.area.3,
                    },
                    first_line: view.first_line,
                    last_line: view.last_line,
                    focused: view.focused,
                })
                .collect()
        }
        fn ui(&mut self) -> UiState {
            let ui = State::ui_state(self);
            UiState {
                mode: to_mode(ui.mode),
                completion_menu: ui.completion_menu,
                prompt: ui.prompt,
                picker: ui.picker,
                popup: ui.popup,
                focused_panel: ui.focused_panel,
                pending_keys: ui.pending_keys,
            }
        }
    }
}

mod services_impl {
    use super::*;
    use v0_5::helix::plugin::services::{Host, ServiceInfo};

    impl Host for State {
        fn register(
            &mut self,
            name: String,
            version: String,
            callers: Vec<String>,
        ) -> std::result::Result<(), String> {
            State::register_service(self, name, version, callers)
        }
        fn discover(&mut self, name: String) -> Vec<ServiceInfo> {
            State::discover_services(self, &name)
                .into_iter()
                .map(|service| ServiceInfo {
                    plugin: service.plugin,
                    name: service.name,
                    version: service.version,
                })
                .collect()
        }
        fn call(
            &mut self,
            service: String,
            version: String,
            method: String,
            payload: String,
        ) -> std::result::Result<u32, String> {
            State::call_service(self, service, version, method, payload)
        }
    }
}

mod floats_impl {
    use super::*;
    use v0_5::helix::plugin::floats::{Float, Host, Position};

    impl Host for State {
        fn show(&mut self, float: Float) -> std::result::Result<(), String> {
            let items = float
                .items
                .into_iter()
                .map(|item| crate::SidebarItem {
                    text: item.text,
                    scope: item.scope,
                    depth: item.depth.min(u16::MAX as u32) as u16,
                    command: item.command.map(|command| (command, item.args)),
                })
                .collect();
            State::show_float(
                self,
                v05::FloatSpec {
                    title: float.title,
                    items,
                    width: float.width,
                    height: float.height,
                    position: match float.position {
                        Position::Center => FloatPosition::Center,
                        Position::Cursor => FloatPosition::Cursor,
                        Position::TopRight => FloatPosition::TopRight,
                    },
                    selected: float.selected,
                    focus: float.focus,
                },
            )
        }
        fn close(&mut self) {
            State::close_sidebar(self, crate::Dock::Float)
        }
        fn focus(&mut self, focused: bool) {
            State::focus_sidebar(self, crate::Dock::Float, focused)
        }
        fn selected(&mut self) -> Option<u32> {
            State::sidebar_selected(self, crate::Dock::Float)
        }
    }
}

mod marks_impl {
    use super::*;
    use v0_5::helix::plugin::marks::{Gravity as G, Host};

    impl Host for State {
        fn set(&mut self, doc: u32, position: u32, gravity: G) -> std::result::Result<u64, String> {
            let gravity = match gravity {
                G::Before => Gravity::Before,
                G::After => Gravity::After,
            };
            State::set_mark(self, doc, position, gravity)
        }
        fn get(&mut self, id: u64) -> Option<(u32, u32)> {
            State::get_mark(self, id)
        }
        fn remove(&mut self, id: u64) {
            State::remove_mark(self, id)
        }
        fn list(&mut self, doc: u32) -> Vec<(u64, u32)> {
            State::list_marks(self, doc)
        }
    }
}

mod decorations2_impl {
    use super::*;
    use v0_5::helix::plugin::decorations::{Annotation, Highlight};
    use v0_5::helix::plugin::decorations2::{Host, VirtualLine};

    impl Host for State {
        fn set_virtual_lines(
            &mut self,
            doc: u32,
            layer: String,
            lines: Vec<VirtualLine>,
        ) -> std::result::Result<(), String> {
            let lines = lines
                .into_iter()
                .map(|line| crate::PluginVirtualLine {
                    line: line.line as usize,
                    text: line.text,
                    scope: line.scope,
                })
                .collect();
            State::set_virtual_lines(self, doc, layer, lines)
        }
        fn patch_highlights(
            &mut self,
            doc: u32,
            layer: String,
            from: u32,
            to: u32,
            highlights: Vec<Highlight>,
        ) -> std::result::Result<(), String> {
            let highlights = highlights
                .into_iter()
                .map(|h| (h.from, h.to, h.scope))
                .collect();
            State::patch_highlights(self, doc, layer, from, to, highlights)
        }
        fn patch_annotations(
            &mut self,
            doc: u32,
            layer: String,
            from: u32,
            to: u32,
            annotations: Vec<Annotation>,
        ) -> std::result::Result<(), String> {
            let annotations = annotations
                .into_iter()
                .map(|a| (a.position, a.text, a.scope))
                .collect();
            State::patch_annotations(self, doc, layer, from, to, annotations)
        }
    }
}

mod sessions_impl {
    use super::*;
    use v0_5::helix::plugin::process::SpawnOptions;
    use v0_5::helix::plugin::sessions::Host;

    impl Host for State {
        fn start(
            &mut self,
            argv: Vec<String>,
            options: SpawnOptions,
        ) -> std::result::Result<u32, String> {
            State::start_session(self, argv, options.cwd, options.stdin, options.timeout_ms)
        }
        fn write(&mut self, id: u32, data: String) -> std::result::Result<(), String> {
            State::session_write(self, id, data)
        }
        fn close_input(&mut self, id: u32) -> std::result::Result<(), String> {
            State::session_close_input(self, id)
        }
    }
}

impl v0_5::helix::plugin::progress::Host for State {
    fn begin(&mut self, title: String) -> u32 {
        State::progress_begin(self, title)
    }
    fn report(&mut self, token: u32, message: String, percent: Option<u8>) {
        State::progress_report(self, token, message, percent)
    }
    fn end(&mut self, token: u32) {
        State::progress_end(self, token)
    }
}

impl v0_5::helix::plugin::watch::Host for State {
    fn add(&mut self, path: String, recursive: bool) -> std::result::Result<u32, String> {
        State::watch(self, path, recursive)
    }
}

impl v0_5::helix::plugin::lsp::Host for State {
    fn servers(&mut self, doc: u32) -> std::result::Result<Vec<String>, String> {
        State::lsp_servers(self, doc)
    }
    fn request(
        &mut self,
        doc: u32,
        server: Option<String>,
        method: String,
        params: String,
    ) -> std::result::Result<u32, String> {
        State::lsp_request(self, doc, server, method, params)
    }
}

/// `hooks.on-key`.
pub(crate) async fn key(
    instance: &v0_5::HelixPlugin,
    store: &mut Store<State>,
    name: &str,
    keys: &str,
    count: Option<u32>,
) -> Result<KeyOutcome> {
    use v0_5::exports::helix::plugin::hooks::KeyOutcome as K;
    let outcome = instance
        .helix_plugin_hooks()
        .call_on_key(store, name, keys, count)
        .await?;
    Ok(match outcome {
        K::Handled => KeyOutcome::Handled,
        K::PassThrough => KeyOutcome::PassThrough,
    })
}

fn hook_outcome(outcome: v0_5::exports::helix::plugin::hooks::HookOutcome) -> HookOutcome {
    use v0_5::exports::helix::plugin::hooks::HookOutcome as H;
    match outcome {
        H::Proceed => HookOutcome::Proceed,
        H::Cancel(reason) => HookOutcome::Cancel(reason),
    }
}

/// `hooks.on-pre-command`.
pub(crate) async fn pre_command(
    instance: &v0_5::HelixPlugin,
    store: &mut Store<State>,
    command: &str,
) -> Result<HookOutcome> {
    instance
        .helix_plugin_hooks()
        .call_on_pre_command(store, command)
        .await
        .map(hook_outcome)
}

/// `hooks.on-pre-save`.
pub(crate) async fn pre_save(
    instance: &v0_5::HelixPlugin,
    store: &mut Store<State>,
    doc: u32,
) -> Result<HookOutcome> {
    instance
        .helix_plugin_hooks()
        .call_on_pre_save(store, doc)
        .await
        .map(hook_outcome)
}

/// `hooks.on-service-call`.
pub(crate) async fn service_call(
    instance: &v0_5::HelixPlugin,
    store: &mut Store<State>,
    caller: &str,
    service: &str,
    method: &str,
    payload: &str,
) -> Result<std::result::Result<String, String>> {
    instance
        .helix_plugin_hooks()
        .call_on_service_call(store, caller, service, method, payload)
        .await
}

/// `events2.on-event`.
pub(crate) async fn event2(
    instance: &v0_5::HelixPlugin,
    store: &mut Store<State>,
    event: &Event2,
) -> Result<()> {
    use v0_5::exports::helix::plugin::events2::{
        CloseReason as R, CompletionEvent as C, Event as E, PluginState as P,
    };
    let event = match event {
        Event2::BufferEnter(doc, view) => E::BufferEnter((*doc, *view)),
        Event2::BufferLeave(doc, view) => E::BufferLeave((*doc, *view)),
        Event2::ViewEnter(view) => E::ViewEnter(*view),
        Event2::ViewLeave(view) => E::ViewLeave(*view),
        Event2::CompletionMenu(event) => E::CompletionMenu(match event {
            CompletionEvent::Opened(n) => C::Opened(*n),
            CompletionEvent::Updated(n) => C::Updated(*n),
            CompletionEvent::Confirmed(label) => C::Confirmed(label.clone()),
            CompletionEvent::Closed(reason) => C::Closed(match reason {
                CloseReason::Aborted => R::Aborted,
                CloseReason::Confirmed => R::Confirmed,
                CloseReason::Dismissed => R::Dismissed,
                CloseReason::ModeChange => R::ModeChange,
                CloseReason::Replaced => R::Replaced,
            }),
        }),
        Event2::ConfigChange(config) => E::ConfigChange(config.clone()),
        Event2::PluginChange(plugin, state) => E::PluginChange((
            plugin.clone(),
            match state {
                PluginState::Loaded => P::Loaded,
                PluginState::Unloaded => P::Unloaded,
                PluginState::Disabled => P::Disabled,
            },
        )),
        Event2::FileChange(id, paths) => E::FileChange((*id, paths.clone())),
        Event2::Shutdown => E::Shutdown,
    };
    instance
        .helix_plugin_events2()
        .call_on_event(store, &event)
        .await
}

/// `callbacks2`.
pub(crate) async fn callback2(
    instance: &v0_5::HelixPlugin,
    store: &mut Store<State>,
    callback: &v05::Callback2,
) -> Result<()> {
    let callbacks = instance.helix_plugin_callbacks2();
    match callback {
        v05::Callback2::CommandDone { id, result } => {
            callbacks
                .call_on_command_done(
                    store,
                    *id,
                    result.as_ref().map(|_| ()).map_err(String::as_str),
                )
                .await
        }
        v05::Callback2::BatchDone { id, results } => {
            callbacks.call_on_batch_done(store, *id, results).await
        }
        v05::Callback2::ServiceReply { id, result } => {
            callbacks
                .call_on_service_reply(
                    store,
                    *id,
                    result.as_ref().map(String::as_str).map_err(String::as_str),
                )
                .await
        }
        v05::Callback2::LspReply { id, result } => {
            callbacks
                .call_on_lsp_reply(
                    store,
                    *id,
                    result.as_ref().map(String::as_str).map_err(String::as_str),
                )
                .await
        }
    }
}
