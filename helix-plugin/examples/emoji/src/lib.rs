//! Example plugin: a completion source (API 0.3). Type `:` and part of a
//! name, e.g. `:smi`, and the completion menu offers `😄 smile`; choosing
//! it replaces `:smi` with the emoji. Plugins' items join the menu with the
//! language servers' and the editor's own.

use helix_plugin_api::{
    completion::{self, item, CompletionItem},
    editor, register_plugin, DocumentId, EventKind, Plugin, Registration,
};

const EMOJI: &[(&str, &str)] = &[
    ("smile", "😄"),
    ("grin", "😁"),
    ("joy", "😂"),
    ("wink", "😉"),
    ("heart", "❤️"),
    ("thumbsup", "👍"),
    ("thumbsdown", "👎"),
    ("clap", "👏"),
    ("fire", "🔥"),
    ("rocket", "🚀"),
    ("sparkles", "✨"),
    ("tada", "🎉"),
    ("eyes", "👀"),
    ("thinking", "🤔"),
    ("warning", "⚠️"),
    ("check", "✅"),
    ("x", "❌"),
    ("bug", "🐛"),
    ("memo", "📝"),
    ("bulb", "💡"),
    ("lock", "🔒"),
    ("zap", "⚡"),
    ("coffee", "☕"),
    ("wave", "👋"),
    ("pray", "🙏"),
    ("100", "💯"),
];

/// The completions for `typed` (what follows the `:`), starting at `from`.
fn complete(typed: &str, from: u32) -> Vec<CompletionItem> {
    EMOJI
        .iter()
        .filter(|(name, _)| name.starts_with(typed))
        .map(|(name, emoji)| {
            let mut item = item(format!(":{name}"));
            item.insert = Some((*emoji).to_string());
            item.kind = Some((*emoji).to_string());
            item.replace_from = Some(from);
            item
        })
        .collect()
}

/// `:name` at the end of `line`, if any: where the `:` is, and the name.
fn emoji_prefix(line: &str) -> Option<(usize, &str)> {
    let colon = line.rfind(':')?;
    let typed = &line[colon + 1..];
    typed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
        .then_some((colon, typed))
}

struct Emoji;

impl Plugin for Emoji {
    fn init(_config: String) -> Result<Registration, String> {
        Ok(Registration {
            commands: Vec::new(),
            events: vec![EventKind::Completion],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        Err(format!("unknown command {name}"))
    }

    fn on_completion(id: u32, doc: DocumentId, position: u32) {
        // Only the line up to the cursor matters.
        let start = position.saturating_sub(40);
        let items = editor::text_range(doc, start, position)
            .ok()
            .and_then(|before| {
                let line = before.rsplit('\n').next()?;
                let (colon, typed) = emoji_prefix(line)?;
                let from = position - line[colon..].chars().count() as u32;
                Some(complete(typed, from))
            })
            .unwrap_or_default();
        // Always answer, so the menu doesn't wait.
        let _ = completion::complete(id, &items);
    }
}

register_plugin!(Emoji);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn completes_after_a_colon() {
        assert_eq!(emoji_prefix("say :smi"), Some((4, "smi")));
        assert_eq!(emoji_prefix("a: b"), None);
        let items = complete("th", 4);
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, [":thumbsup", ":thumbsdown", ":thinking"]);
        assert_eq!(items[0].insert.as_deref(), Some("👍"));
        assert_eq!(items[0].replace_from, Some(4));
    }
}
