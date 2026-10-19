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

#[cfg(not(feature = "api-v0-1"))]
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

pub use bindings::helix::plugin::types::{
    CommandInfo, DocumentId, Edit, EventKind, Mode, Range, Registration,
};

/// Functions provided by the editor.
pub mod editor {
    pub use crate::bindings::helix::plugin::host::*;
    #[cfg(not(feature = "api-v0-1"))]
    pub use crate::requests::{kill, pick, run_interactive, spawn};
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

        $crate::__export_callbacks!(__HelixPluginExport);
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
    ui::{Item, Picker, PickerResult, Preview},
};
#[cfg(not(feature = "api-v0-1"))]
pub use requests::{Choice, InteractiveOptions, SpawnOptions};

#[cfg(feature = "api-v0-1")]
#[doc(hidden)]
#[macro_export]
macro_rules! __export_callbacks {
    ($export:ty) => {};
}

#[cfg(not(feature = "api-v0-1"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __export_callbacks {
    ($export:ty) => {
        impl $crate::bindings::exports::helix::plugin::callbacks::Guest for $export {
            fn on_process_done(id: $crate::RequestId, result: Result<$crate::Output, String>) {
                $crate::requests::process_done(id, result)
            }
            fn on_picker_done(id: $crate::RequestId, result: $crate::PickerResult) {
                $crate::requests::picker_done(id, result)
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
