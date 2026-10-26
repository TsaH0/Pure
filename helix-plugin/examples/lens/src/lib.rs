//! Example plugin (API 0.5): below each Rust `fn`, a virtual line saying
//! how often the file uses it, like a code lens. It shows most of 0.5:
//!
//! - a settings schema in `plugin.toml` (checked by the editor, defaults
//!   filled in) and `config-change` to follow `:config-reload`;
//! - a key bound to a closure (`space L` toggles the lenses);
//! - subscriptions filtered to Rust documents;
//! - virtual lines (`decorations2`);
//! - a service other plugins can call: `lens.uses`, method `count`, payload
//!   a function name as a JSON string, reply the number of uses.

use std::cell::{Cell, RefCell};

use helix_plugin_api::{
    decorations2::{self, VirtualLine},
    editor,
    keymaps::{self, binding},
    register_plugin, services,
    subscriptions::{self, EventType},
    DocumentId, Event, KeyOutcome, Mode, Plugin, Registration,
};

/// Larger documents get no lenses.
const MAX_CHARS: usize = 1_000_000;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(true) };
    static MIN_USES: Cell<u64> = const { Cell::new(0) };
    static SCOPE: RefCell<String> = RefCell::new("comment".into());
}

fn configure(config: &str) {
    let config: serde_json::Value = serde_json::from_str(config).unwrap_or_default();
    // The schema guarantees the types and fills in defaults.
    if let Some(enabled) = config["enabled"].as_bool() {
        ENABLED.set(enabled);
    }
    if let Some(min) = config["min-uses"].as_u64() {
        MIN_USES.set(min);
    }
    if let Some(scope) = config["scope"].as_str() {
        SCOPE.set(scope.to_string());
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `(line, name)` of each function defined in `text`.
fn functions(text: &str) -> Vec<(u32, String)> {
    text.lines()
        .enumerate()
        .filter_map(|(line, content)| {
            let rest = content.trim_start();
            let rest = rest.strip_prefix("pub ").unwrap_or(rest);
            let rest = rest.strip_prefix("pub(crate) ").unwrap_or(rest);
            let rest = rest.strip_prefix("async ").unwrap_or(rest);
            let name: String = rest.strip_prefix("fn ")?.chars().take_while(|c| is_word(*c)).collect();
            (!name.is_empty()).then_some((line as u32, name))
        })
        .collect()
}

/// Whole-word uses of `name` in `text`, the definition excluded.
fn uses(text: &str, name: &str) -> usize {
    text.match_indices(name)
        .filter(|(byte, _)| {
            let before = text[..*byte].chars().next_back();
            let after = text[byte + name.len()..].chars().next();
            !before.is_some_and(is_word) && !after.is_some_and(is_word)
        })
        .count()
        .saturating_sub(1)
}

fn update(doc: DocumentId) -> Result<(), String> {
    if !ENABLED.get() {
        return decorations2::set_virtual_lines(doc, "lens", &[]);
    }
    let text = editor::text(doc)?;
    if text.len() > MAX_CHARS {
        return decorations2::set_virtual_lines(doc, "lens", &[]);
    }
    let scope = SCOPE.with_borrow(|s| s.clone());
    let lines: Vec<VirtualLine> = functions(&text)
        .into_iter()
        .filter_map(|(line, name)| {
            let count = uses(&text, &name);
            (count as u64 >= MIN_USES.get()).then(|| VirtualLine {
                line,
                text: match count {
                    0 => "    ↳ unused in this file".to_string(),
                    1 => "    ↳ 1 use".to_string(),
                    n => format!("    ↳ {n} uses"),
                },
                scope: scope.clone(),
            })
        })
        .collect();
    decorations2::set_virtual_lines(doc, "lens", &lines)
}

fn update_current() -> Result<(), String> {
    match editor::current_document() {
        Some(doc) if editor::document_language(doc)?.as_deref() == Some("rust") => update(doc),
        _ => Ok(()),
    }
}

struct Lens;

impl Plugin for Lens {
    fn init(config: String) -> Result<Registration, String> {
        configure(&config);
        let rust = subscriptions::Filter {
            language: Some("rust".into()),
            ..subscriptions::any()
        };
        // Rust documents only: other changes never reach the plugin.
        subscriptions::subscribe(EventType::DocumentChange, &rust)?;
        subscriptions::subscribe(EventType::DocumentOpen, &rust)?;
        subscriptions::subscribe(EventType::BufferEnter, &rust)?;
        subscriptions::subscribe(EventType::ConfigChange, &subscriptions::any())?;
        keymaps::bind(
            binding(Mode::Normal, "space L", "toggle", "Toggle code lenses"),
            |_keys, _count| {
                ENABLED.set(!ENABLED.get());
                update_current()?;
                Ok(KeyOutcome::Handled)
            },
        )?;
        services::provide("uses", "1.0.0", &[], |_caller, method, payload| {
            if method != "count" {
                return Err(format!("no method '{method}'"));
            }
            let name: String = serde_json::from_str(payload).map_err(|err| err.to_string())?;
            let doc = editor::current_document().ok_or("no document")?;
            Ok(uses(&editor::text(doc)?, &name).to_string())
        })?;
        Ok(Registration {
            commands: Vec::new(),
            events: Vec::new(),
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        Err(format!("unknown command {name}"))
    }

    fn on_document_open(doc: DocumentId) {
        let _ = update(doc);
    }
    fn on_document_change(doc: DocumentId) {
        let _ = update(doc);
    }

    fn on_event(event: Event) {
        match event {
            Event::BufferEnter((doc, _)) => {
                let _ = update(doc);
            }
            Event::ConfigChange(config) => {
                configure(&config);
                let _ = update_current();
            }
            _ => {}
        }
    }
}

register_plugin!(Lens);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn functions_and_uses() {
        let text = "fn main() {\n    helper(1);\n}\npub fn helper(x: u8) {}\nfn helpers() {}\n";
        assert_eq!(
            functions(text),
            [(0, "main".into()), (3, "helper".into()), (4, "helpers".into())]
        );
        assert_eq!(uses(text, "helper"), 1);
        assert_eq!(uses(text, "main"), 0);
    }
}
