//! SDK for writing [Pure](https://github.com/TsaH0/helix-editor-custom) plugins in Rust.
//!
//! ```ignore
//! use helix_plugin_api::{editor, register_plugin, CommandInfo, Plugin, Registration};
//!
//! struct Hello;
//!
//! impl Plugin for Hello {
//!     fn init(_config: String) -> Result<Registration, String> {
//!         Ok(Registration {
//!             commands: vec![CommandInfo { name: "hello".into(), doc: "Say hello".into() }],
//!             events: vec![],
//!         })
//!     }
//!
//!     fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
//!         editor::set_status(&format!("hello from {name}"));
//!         Ok(())
//!     }
//! }
//!
//! register_plugin!(Hello);
//! ```
//!
//! Build with `cargo build --release --target wasm32-wasip2` and put the
//! resulting `.wasm` next to a `plugin.toml` in `~/.config/helix/plugins/<name>/`.

#[cfg(any(
    all(feature = "api-v0-1", feature = "api-v0-2"),
    all(feature = "api-v0-1", feature = "api-v0-3"),
    all(feature = "api-v0-2", feature = "api-v0-3"),
))]
compile_error!("select only one legacy plugin ABI feature");

#[cfg(feature = "api-v0-1")]
#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "../wit/v0.1",
        world: "helix-plugin",
        pub_export_macro: true,
        export_macro_name: "__export_helix_plugin",
        default_bindings_module: "helix_plugin_api::bindings",
    });
}

#[cfg(all(not(feature = "api-v0-1"), feature = "api-v0-2"))]
#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "../wit/v0.2",
        world: "helix-plugin",
        pub_export_macro: true,
        export_macro_name: "__export_helix_plugin",
        default_bindings_module: "helix_plugin_api::bindings",
    });
}

#[cfg(all(not(any(feature = "api-v0-1", feature = "api-v0-2")), feature = "api-v0-3"))]
#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "../wit/v0.3",
        world: "helix-plugin",
        pub_export_macro: true,
        export_macro_name: "__export_helix_plugin",
        default_bindings_module: "helix_plugin_api::bindings",
    });
}

#[cfg(not(any(feature = "api-v0-1", feature = "api-v0-2", feature = "api-v0-3")))]
#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "../wit/v0.4",
        world: "helix-plugin",
        pub_export_macro: true,
        export_macro_name: "__export_helix_plugin",
        default_bindings_module: "helix_plugin_api::bindings",
    });
}

pub use bindings::helix::plugin::types::{
    CommandInfo, DocumentId, Edit, EventKind, Mode, Range, Registration,
};

/// Functions provided by the editor.
pub mod editor {
    pub use crate::bindings::helix::plugin::host::*;
    #[cfg(not(any(feature = "api-v0-1", feature = "api-v0-2")))]
    pub use crate::requests::{
        cancel_timer, close_popup, popup_with_options, push_items_for_query, run_background,
        set_items_for_query, set_timer, spawn_stream,
    };
    #[cfg(not(feature = "api-v0-1"))]
    pub use crate::requests::{
        kill, pick, pick_live, popup, prompt, push_items, run_interactive, set_items, spawn,
    };
}

/// How documents look (API 0.3): highlights, virtual text, gutter signs and
/// statusline segments, in layers the plugin names. Setting a layer replaces
/// it; an empty list clears it. Positions follow the user's edits until the
/// plugin sets the layer again.
#[cfg(not(any(feature = "api-v0-1", feature = "api-v0-2")))]
pub mod decorations {
    pub use crate::bindings::helix::plugin::decorations::*;

    /// A highlight of chars `from..to` with a theme scope (e.g. `warning`).
    pub fn highlight(from: u32, to: u32, scope: &str) -> Highlight {
        Highlight {
            from,
            to,
            scope: scope.into(),
        }
    }

    /// Virtual text before the char at `position`.
    pub fn annotation(position: u32, text: impl Into<String>, scope: &str) -> Annotation {
        Annotation {
            position,
            text: text.into(),
            scope: scope.into(),
        }
    }

    /// `text` drawn over the chars from `position` (one char over each).
    pub fn overlay(position: u32, text: impl Into<String>, scope: &str) -> Annotation {
        annotation(position, text, scope)
    }

    /// A one-character sign on a 0-based line.
    pub fn sign(line: u32, text: impl Into<String>, scope: &str) -> Sign {
        Sign {
            line,
            text: text.into(),
            scope: scope.into(),
        }
    }
}

/// Diagnostics for linters (API 0.3): shown like a language server's, in
/// the gutter, inline, by `]d`/`[d`, in the pickers and the statusline.
#[cfg(not(any(feature = "api-v0-1", feature = "api-v0-2")))]
pub mod diagnostics {
    pub use crate::bindings::helix::plugin::diagnostics::*;

    /// A diagnostic over chars `from..to`.
    pub fn diagnostic(from: u32, to: u32, severity: Severity, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            from,
            to,
            severity,
            message: message.into(),
            code: None,
            source: None,
        }
    }
}

/// File operations through the editor (API 0.3): open buffers and language
/// servers follow renames. Needs `filesystem` and `allow-files = "write"`.
/// API 0.3 has `create_path`, `rename_path` and `delete_path`, carried out
/// after the call; API 0.4 has [`files::apply`] with results.
#[cfg(not(any(feature = "api-v0-1", feature = "api-v0-2")))]
pub mod files {
    #[cfg(feature = "api-v0-3")]
    pub use crate::bindings::helix::plugin::files::*;
    #[cfg(not(feature = "api-v0-3"))]
    pub use crate::bindings::helix::plugin::files::{roots, CreateOp, FileOp, MoveOp};
    #[cfg(not(feature = "api-v0-3"))]
    pub use crate::requests::apply_files as apply;
}

/// Completion sources (API 0.3): subscribe to `EventKind::Completion`,
/// implement [`Plugin::on_completion`] and answer with [`complete`].
#[cfg(not(any(feature = "api-v0-1", feature = "api-v0-2")))]
pub mod completion {
    pub use crate::bindings::helix::plugin::completion::*;

    /// An item inserting its label in place of the word before the cursor.
    pub fn item(label: impl Into<String>) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            insert: None,
            kind: None,
            documentation: None,
            replace_from: None,
        }
    }
}

/// A Pure plugin. Only `init` and `run_command` have to be implemented; the
/// event handlers are only called for the events returned from `init`.
pub trait Plugin {
    /// Called once when the plugin is loaded. `config` is the plugin's table in
    /// `plugins.toml`, as JSON.
    fn init(config: String) -> Result<Registration, String>;

    /// Run one of the commands returned from `init`.
    fn run_command(name: &str, args: Vec<String>) -> Result<(), String>;

    /// Called when a document opens, if subscribed.
    fn on_document_open(_doc: DocumentId) {}
    /// Called after debounced document edits, if subscribed.
    fn on_document_change(_doc: DocumentId) {}
    /// Called after the editor changes mode, if subscribed.
    fn on_mode_switch(_old: Mode, _new: Mode) {}
    /// Called after a document was saved, if subscribed (API 0.2).
    fn on_document_save(_doc: DocumentId) {}
    /// Called after a document was closed, if subscribed (API 0.2). Only its
    /// id is known.
    fn on_document_close(_doc: DocumentId) {}
    /// Called after the selections changed (debounced), if subscribed
    /// (API 0.2).
    fn on_selection_change(_doc: DocumentId) {}
    /// Called after a key command, if subscribed (API 0.3).
    fn on_post_command(_command: &str) {}
    /// Runs in the plugin's *background instance* (API 0.3), for tasks
    /// started with `editor::run_background`: a separate instance with its
    /// own memory, on another thread, without editor access. Compute from
    /// `input` only and return the result.
    fn run_background(task: &str, _input: String) -> Result<String, String> {
        Err(format!("no background task '{task}'"))
    }
    /// Called when the terminal gains focus, if subscribed (API 0.3).
    fn on_focus_gained() {}
    /// Called when the terminal loses focus, if subscribed (API 0.3).
    fn on_focus_lost() {}
    /// Called by the editor's existing idle timer, if subscribed (API 0.3).
    fn on_editor_idle() {}
    /// The editor wants completions at `position` in `doc`, if subscribed to
    /// `EventKind::Completion` (API 0.3). Answer with
    /// `completion::complete(id, ...)`, now or from a later callback.
    fn on_completion(_id: u32, _doc: DocumentId, _position: u32) {}
    /// `:write` on one of the plugin's buffers (`editor::open_buffer`), as a
    /// command (API 0.3). Call `editor::mark_saved` once applied.
    fn on_buffer_write(_doc: DocumentId) -> Result<(), String> {
        Err("this buffer can't be written".into())
    }
}

/// Export a type implementing [`Plugin`] as the plugin.
#[macro_export]
macro_rules! register_plugin {
    ($plugin:ty) => {
        struct __HelixPluginExport;

        impl $crate::bindings::exports::helix::plugin::plugin::Guest for __HelixPluginExport {
            fn init(config: String) -> Result<$crate::Registration, String> {
                <$plugin as $crate::Plugin>::init(config)
            }
            fn run_command(name: String, args: Vec<String>) -> Result<(), String> {
                $crate::__run_command(|| <$plugin as $crate::Plugin>::run_command(&name, args))
            }
            fn on_document_open(doc: $crate::DocumentId) {
                <$plugin as $crate::Plugin>::on_document_open(doc)
            }
            fn on_document_change(doc: $crate::DocumentId) {
                <$plugin as $crate::Plugin>::on_document_change(doc)
            }
            fn on_mode_switch(old: $crate::Mode, new: $crate::Mode) {
                <$plugin as $crate::Plugin>::on_mode_switch(old, new)
            }
        }

        $crate::__export_callbacks!(__HelixPluginExport, $plugin);
        $crate::bindings::__export_helix_plugin!(__HelixPluginExport with_types_in $crate::bindings);
    };
}

/// Convenience helpers. The editor works with char offsets while Rust strings
/// are indexed by bytes.
pub mod text {
    /// Number of chars in `s`.
    pub fn char_len(s: &str) -> u32 {
        s.chars().count() as u32
    }

    /// Byte index of char offset `char_idx` in `s` (clamped to `s.len()`).
    pub fn char_to_byte(s: &str, char_idx: u32) -> usize {
        s.char_indices()
            .nth(char_idx as usize)
            .map_or(s.len(), |(byte, _)| byte)
    }

    /// The text between char offsets `from` and `to`.
    pub fn slice(s: &str, from: u32, to: u32) -> &str {
        &s[char_to_byte(s, from)..char_to_byte(s, to)]
    }

    /// Char offsets of the start of each line, plus the end of the text.
    pub fn line_starts(s: &str) -> Vec<u32> {
        let mut starts = vec![0];
        let mut offset = 0;
        for c in s.chars() {
            offset += 1;
            if c == '\n' {
                starts.push(offset);
            }
        }
        if *starts.last().unwrap() != offset {
            starts.push(offset);
        }
        starts
    }
}

#[cfg(not(feature = "api-v0-1"))]
#[doc(hidden)]
pub mod requests;

#[cfg(not(feature = "api-v0-1"))]
pub use bindings::helix::plugin::{
    process::Output,
    types::{OpenAction, RequestId},
    ui::{Item, Picker, PickerResult, Preview, Prompt},
};
#[cfg(not(any(feature = "api-v0-1", feature = "api-v0-2")))]
pub use requests::PopupOptions;
#[cfg(not(feature = "api-v0-1"))]
pub use requests::{Choice, InteractiveOptions, SpawnOptions};

#[cfg(feature = "api-v0-1")]
#[doc(hidden)]
#[macro_export]
macro_rules! __export_callbacks {
    ($export:ty, $plugin:ty) => {};
}

#[cfg(all(not(feature = "api-v0-1"), feature = "api-v0-2"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __export_callbacks {
    ($export:ty, $plugin:ty) => {
        impl $crate::bindings::exports::helix::plugin::callbacks::Guest for $export {
            fn on_process_done(id: $crate::RequestId, result: Result<$crate::Output, String>) {
                $crate::requests::process_done(id, result)
            }
            fn on_picker_done(id: $crate::RequestId, result: $crate::PickerResult) {
                $crate::requests::picker_done(id, result)
            }
            fn on_picker_query(id: $crate::RequestId, query: String) {
                $crate::requests::picker_query(id, query)
            }
            fn on_prompt_done(id: $crate::RequestId, answer: Option<String>) {
                $crate::requests::prompt_done(id, answer)
            }
        }
        impl $crate::bindings::exports::helix::plugin::events::Guest for $export {
            fn on_document_save(doc: $crate::DocumentId) {
                <$plugin as $crate::Plugin>::on_document_save(doc)
            }
            fn on_document_close(doc: $crate::DocumentId) {
                <$plugin as $crate::Plugin>::on_document_close(doc)
            }
            fn on_selection_change(doc: $crate::DocumentId) {
                <$plugin as $crate::Plugin>::on_selection_change(doc)
            }
        }
    };
}

#[cfg(not(any(feature = "api-v0-1", feature = "api-v0-2")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __export_callbacks {
    ($export:ty, $plugin:ty) => {
        impl $crate::bindings::exports::helix::plugin::callbacks::Guest for $export {
            fn on_process_done(id: $crate::RequestId, result: Result<$crate::Output, String>) {
                $crate::requests::process_done(id, result)
            }
            fn on_process_output(id: $crate::RequestId, chunk: String) {
                $crate::requests::process_output(id, chunk)
            }
            fn on_picker_done(id: $crate::RequestId, result: $crate::PickerResult) {
                $crate::requests::picker_done(id, result)
            }
            fn on_picker_query(id: $crate::RequestId, generation: u64, query: String) {
                $crate::requests::picker_query(id, generation, query)
            }
            fn on_popup_close(id: $crate::RequestId) {
                $crate::requests::popup_closed(id)
            }
            fn on_prompt_done(id: $crate::RequestId, answer: Option<String>) {
                $crate::requests::prompt_done(id, answer)
            }
            fn on_background_done(id: $crate::RequestId, result: Result<String, String>) {
                $crate::requests::background_done(id, result)
            }
            fn on_timer(id: $crate::RequestId) {
                $crate::requests::timer_fired(id)
            }
            $crate::__files_done!();
        }
        impl $crate::bindings::exports::helix::plugin::background::Guest for $export {
            fn run(task: String, input: String) -> Result<String, String> {
                <$plugin as $crate::Plugin>::run_background(&task, input)
            }
        }
        impl $crate::bindings::exports::helix::plugin::events::Guest for $export {
            fn on_document_save(doc: $crate::DocumentId) {
                <$plugin as $crate::Plugin>::on_document_save(doc)
            }
            fn on_document_close(doc: $crate::DocumentId) {
                <$plugin as $crate::Plugin>::on_document_close(doc)
            }
            fn on_selection_change(doc: $crate::DocumentId) {
                <$plugin as $crate::Plugin>::on_selection_change(doc)
            }
            fn on_post_command(command: String) {
                <$plugin as $crate::Plugin>::on_post_command(&command)
            }
            fn on_focus_gained() {
                <$plugin as $crate::Plugin>::on_focus_gained()
            }
            fn on_focus_lost() {
                <$plugin as $crate::Plugin>::on_focus_lost()
            }
            fn on_editor_idle() {
                <$plugin as $crate::Plugin>::on_editor_idle()
            }
            fn on_completion(id: $crate::RequestId, doc: $crate::DocumentId, position: u32) {
                <$plugin as $crate::Plugin>::on_completion(id, doc, position)
            }
            fn on_buffer_write(doc: $crate::DocumentId) {
                // Like a command: errors show, requests made here commit.
                if let Err(err) =
                    $crate::__run_command(|| <$plugin as $crate::Plugin>::on_buffer_write(doc))
                {
                    $crate::editor::set_error(&err);
                }
            }
        }
    };
}

#[doc(hidden)]
pub fn __run_command(f: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    #[cfg(feature = "api-v0-1")]
    {
        f()
    }
    #[cfg(not(feature = "api-v0-1"))]
    {
        requests::run_command(f)
    }
}

#[cfg(not(any(feature = "api-v0-1", feature = "api-v0-2", feature = "api-v0-3")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __files_done {
    () => {
        fn on_files_done(id: $crate::RequestId, results: Vec<Result<(), String>>) {
            $crate::requests::files_done(id, results)
        }
    };
}

#[cfg(feature = "api-v0-3")]
#[doc(hidden)]
#[macro_export]
macro_rules! __files_done {
    () => {};
}

