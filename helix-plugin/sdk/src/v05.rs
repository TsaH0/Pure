//! API 0.5: key callbacks, the completion menu, subscriptions, commands,
//! editor state, services, floats, marks, more decorations, sessions,
//! progress, file watches and language servers.
//!
//! Closures passed to [`keymaps::bind`], [`services::provide`],
//! [`commands::execute`], [`watch::add`] and the like run when the editor
//! calls back; the matching [`crate::Plugin`] methods handle what no
//! closure claims.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::bindings::helix::plugin;
use crate::RequestId;

pub use crate::bindings::exports::helix::plugin::events2::{
    CloseReason, CompletionEvent, Event, PluginState,
};
pub use crate::bindings::exports::helix::plugin::hooks::{HookOutcome, KeyOutcome};

type KeyHandler = Box<dyn FnMut(&str, Option<u32>) -> Result<KeyOutcome, String>>;
type ServiceHandler = Box<dyn FnMut(&str, &str, &str) -> Result<String, String>>;
type WatchHandler = Box<dyn FnMut(&[String]) -> Result<(), String>>;
type Reply<T> = Box<dyn FnOnce(T) -> Result<(), String>>;

thread_local! {
    static KEYS: RefCell<HashMap<String, KeyHandler>> = RefCell::new(HashMap::new());
    static SERVICES: RefCell<HashMap<String, ServiceHandler>> = RefCell::new(HashMap::new());
    static WATCHES: RefCell<HashMap<RequestId, WatchHandler>> = RefCell::new(HashMap::new());
    static COMMANDS: RefCell<HashMap<RequestId, Reply<Result<(), String>>>> = RefCell::new(HashMap::new());
    static BATCHES: RefCell<HashMap<RequestId, Reply<Vec<Result<(), String>>>>> = RefCell::new(HashMap::new());
    static REPLIES: RefCell<HashMap<RequestId, Reply<Result<String, String>>>> = RefCell::new(HashMap::new());
}

fn report(result: Result<(), String>) {
    if let Err(err) = result {
        crate::editor::set_error(&format!("plugin callback: {err}"));
    }
}

/// Bind keys to plugin callbacks.
pub mod keymaps {
    use super::*;
    pub use plugin::keymaps::{list, set, unset, Binding, Layer};

    /// A binding of `keys` in `mode` to `name`, in the `plugin` layer (above
    /// Pure's defaults, below the user's keys), everywhere.
    pub fn binding(mode: crate::Mode, keys: &str, name: &str, description: &str) -> Binding {
        Binding {
            mode,
            keys: keys.into(),
            name: name.into(),
            description: description.into(),
            layer: Layer::Plugin,
            document: None,
            language: None,
        }
    }

    /// Set `binding` and run `handler(keys, count)` when it is pressed.
    /// Return [`KeyOutcome::PassThrough`] to let the keys do what they
    /// would do without this binding. Like a command: UI and commands are
    /// allowed; an error is shown and counts as handled.
    pub fn bind(
        binding: Binding,
        handler: impl FnMut(&str, Option<u32>) -> Result<KeyOutcome, String> + 'static,
    ) -> Result<(), String> {
        let name = binding.name.clone();
        set(&binding)?;
        KEYS.with_borrow_mut(|keys| keys.insert(name, Box::new(handler)));
        Ok(())
    }

    /// The handler for binding `name`, if [`bind`] set one.
    #[doc(hidden)]
    pub fn handle(name: &str, keys: &str, count: Option<u32>) -> Option<Result<KeyOutcome, String>> {
        let mut handler = KEYS.with_borrow_mut(|handlers| handlers.remove(name))?;
        let result = handler(keys, count);
        KEYS.with_borrow_mut(|handlers| {
            handlers.entry(name.to_string()).or_insert(handler);
        });
        Some(result)
    }
}

/// Drive the completion menu.
pub mod completion_menu {
    pub use super::plugin::completion_menu::*;
}

/// Subscribe to events at any time, with filters.
pub mod subscriptions {
    pub use super::plugin::subscriptions::*;

    /// A filter matching everything.
    pub fn any() -> Filter {
        Filter {
            language: None,
            path_glob: None,
            document: None,
            commands: Vec::new(),
        }
    }
}

/// Commands by name, mode changes and ordered batches.
pub mod commands {
    use super::*;
    pub use plugin::commands::{list, set_mode, Command, CommandKind, Step};

    /// Run command `name` (native, typable or "plugin/command") after this
    /// call, then `done` with its result.
    pub fn execute(
        name: &str,
        args: &[impl AsRef<str>],
        count: Option<u32>,
        done: impl FnOnce(Result<(), String>) -> Result<(), String> + 'static,
    ) -> Result<RequestId, String> {
        let args: Vec<String> = args.iter().map(|a| a.as_ref().to_string()).collect();
        let id = plugin::commands::execute(name, &args, count)?;
        COMMANDS.with_borrow_mut(|commands| commands.insert(id, Box::new(done)));
        Ok(id)
    }

    /// Run `steps` in order after this call; the first failure stops them
    /// and the rest are reported as skipped. Nothing is rolled back.
    pub fn batch(
        steps: &[Step],
        done: impl FnOnce(Vec<Result<(), String>>) -> Result<(), String> + 'static,
    ) -> Result<RequestId, String> {
        let id = plugin::commands::batch(steps)?;
        BATCHES.with_borrow_mut(|batches| batches.insert(id, Box::new(done)));
        Ok(id)
    }

    /// A command step.
    pub fn step(name: &str, args: &[&str], count: Option<u32>) -> Step {
        Step::Command((
            name.into(),
            args.iter().map(|a| a.to_string()).collect(),
            count,
        ))
    }
}

/// Views and UI state.
pub mod state {
    pub use super::plugin::state::*;
}

/// Services for and from other plugins.
pub mod services {
    use super::*;
    pub use plugin::services::{discover, register, ServiceInfo};

    /// Offer service `name` (other plugins call it "<this plugin>.<name>")
    /// from `init`, handled by `handler(caller, method, payload)`. `callers`:
    /// the only plugins allowed to call (empty: any).
    pub fn provide(
        name: &str,
        version: &str,
        callers: &[&str],
        handler: impl FnMut(&str, &str, &str) -> Result<String, String> + 'static,
    ) -> Result<(), String> {
        let callers: Vec<String> = callers.iter().map(|c| c.to_string()).collect();
        register(name, version, &callers)?;
        SERVICES.with_borrow_mut(|services| services.insert(name.to_string(), Box::new(handler)));
        Ok(())
    }

    /// Call `method` of `service` ("plugin.name"), compatible with
    /// `version`; `reply` gets its answer (JSON) or error.
    pub fn call(
        service: &str,
        version: &str,
        method: &str,
        payload: &str,
        reply: impl FnOnce(Result<String, String>) -> Result<(), String> + 'static,
    ) -> Result<RequestId, String> {
        let id = plugin::services::call(service, version, method, payload)?;
        REPLIES.with_borrow_mut(|replies| replies.insert(id, Box::new(reply)));
        Ok(id)
    }

    #[doc(hidden)]
    pub fn handle(
        caller: &str,
        service: &str,
        method: &str,
        payload: &str,
    ) -> Option<Result<String, String>> {
        // "plugin.name": the handler is registered under "name".
        let name = service.split_once('.').map_or(service, |(_, name)| name);
        let mut handler = SERVICES.with_borrow_mut(|services| services.remove(name))?;
        let result = handler(caller, method, payload);
        SERVICES.with_borrow_mut(|services| {
            services.entry(name.to_string()).or_insert(handler);
        });
        Some(result)
    }
}

/// An interactive floating list.
pub mod floats {
    pub use super::plugin::floats::*;
    pub use crate::sidebar::{item, on_enter, SidebarItem};

    /// A centered float titled `title`, `width` x `height`, taking the keys.
    pub fn float(title: &str, items: Vec<SidebarItem>, width: u32, height: u32) -> Float {
        Float {
            title: title.into(),
            items,
            width,
            height,
            position: Position::Center,
            selected: None,
            focus: true,
        }
    }
}

/// Positions that follow edits.
pub mod marks {
    pub use super::plugin::marks::*;
}

/// Virtual lines and partial decoration updates.
pub mod decorations2 {
    pub use super::plugin::decorations2::*;
}

/// Programs that keep reading input.
pub mod sessions {
    use super::*;
    pub use plugin::sessions::{close_input, write};

    /// Start `argv`; `output` gets stdout in batches of lines, `done` the
    /// end (the unfinished last line and stderr).
    pub fn start(
        argv: &[impl AsRef<str>],
        options: crate::SpawnOptions,
        output: impl FnMut(&str) -> Result<(), String> + 'static,
        done: impl FnOnce(Result<crate::Output, String>) -> Result<(), String> + 'static,
    ) -> Result<RequestId, String> {
        let argv: Vec<String> = argv.iter().map(|a| a.as_ref().to_string()).collect();
        let options: plugin::process::SpawnOptions = options.into();
        let id = plugin::sessions::start(&argv, &options)?;
        crate::requests::register_stream(id, output, done);
        Ok(id)
    }
}

/// Progress in the statusline.
pub mod progress {
    pub use super::plugin::progress::*;
}

/// File watches.
pub mod watch {
    use super::*;

    /// Watch `path`; `changed` gets the changed paths, coalesced. Stop with
    /// `editor::cancel`.
    pub fn add(
        path: &str,
        recursive: bool,
        changed: impl FnMut(&[String]) -> Result<(), String> + 'static,
    ) -> Result<RequestId, String> {
        let id = plugin::watch::add(path, recursive)?;
        WATCHES.with_borrow_mut(|watches| watches.insert(id, Box::new(changed)));
        Ok(id)
    }
}

/// Requests to language servers.
pub mod lsp {
    use super::*;
    pub use plugin::lsp::servers;

    /// Send `method` with `params` (JSON) to `server` (default: the first
    /// that supports it); `reply` gets the result (JSON) or the error.
    pub fn request(
        doc: crate::DocumentId,
        server: Option<&str>,
        method: &str,
        params: &str,
        reply: impl FnOnce(Result<String, String>) -> Result<(), String> + 'static,
    ) -> Result<RequestId, String> {
        let id = plugin::lsp::request(doc, server, method, params)?;
        REPLIES.with_borrow_mut(|replies| replies.insert(id, Box::new(reply)));
        Ok(id)
    }
}

/// Forget the closures of request `id` (cancelled).
pub(crate) fn forget(id: RequestId) {
    WATCHES.with_borrow_mut(|watches| watches.remove(&id));
    COMMANDS.with_borrow_mut(|commands| commands.remove(&id));
    BATCHES.with_borrow_mut(|batches| batches.remove(&id));
    REPLIES.with_borrow_mut(|replies| replies.remove(&id));
}

#[doc(hidden)]
pub fn command_done(id: RequestId, result: Result<(), String>) {
    if let Some(done) = COMMANDS.with_borrow_mut(|commands| commands.remove(&id)) {
        report(crate::__run_command(|| done(result)));
    }
}

#[doc(hidden)]
pub fn batch_done(id: RequestId, results: Vec<Result<(), String>>) {
    if let Some(done) = BATCHES.with_borrow_mut(|batches| batches.remove(&id)) {
        report(crate::__run_command(|| done(results)));
    }
}

#[doc(hidden)]
pub fn reply(id: RequestId, result: Result<String, String>) {
    if let Some(reply) = REPLIES.with_borrow_mut(|replies| replies.remove(&id)) {
        report(crate::__run_command(|| reply(result)));
    }
}

/// A watch's changes; `false` if no closure took them.
#[doc(hidden)]
pub fn file_change(id: RequestId, paths: &[String]) -> bool {
    let Some(mut changed) = WATCHES.with_borrow_mut(|watches| watches.remove(&id)) else {
        return false;
    };
    report(changed(paths));
    WATCHES.with_borrow_mut(|watches| {
        watches.entry(id).or_insert(changed);
    });
    true
}

/// `hooks.on-key`: a [`keymaps::bind`] closure, else `Plugin::on_key`.
#[doc(hidden)]
pub fn key(
    name: &str,
    keys: &str,
    count: Option<u32>,
    fallback: impl FnOnce() -> Result<KeyOutcome, String>,
) -> KeyOutcome {
    let mut outcome = KeyOutcome::Handled;
    let result = crate::__run_command(|| {
        outcome = match keymaps::handle(name, keys, count) {
            Some(result) => result?,
            None => fallback()?,
        };
        Ok(())
    });
    if let Err(err) = result {
        crate::editor::set_error(&err);
    }
    outcome
}
