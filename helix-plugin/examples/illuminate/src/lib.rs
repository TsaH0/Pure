//! Example plugin: highlight every use of the word under the cursor, like
//! vim-illuminate. Whole words only, at least two characters; nothing while
//! the cursor isn't on a word. Work is skipped when neither the word nor
//! the text changed (a cursor move inside a word costs one small read).
//!
//! `toggle` turns it off and on. Settings: `scope` (default `ui.highlight`).

use std::cell::{Cell, RefCell};

use helix_plugin_api::{
    decorations::{self, highlight},
    editor, register_plugin, CommandInfo, DocumentId, EventKind, Plugin, Registration,
};

/// Larger documents aren't illuminated (each change would read them all).
const MAX_CHARS: u32 = 2_000_000;
/// Most uses highlighted.
const MAX_USES: usize = 10_000;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(true) };
    static SCOPE: RefCell<String> = RefCell::new("ui.highlight".into());
    /// What is highlighted now: (document, revision, word).
    static SHOWN: RefCell<Option<(DocumentId, u64, String)>> = const { RefCell::new(None) };
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word around char `at` of `text` (`at` may be just after it).
fn word_at(text: &str, at: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let at = if chars.get(at).is_some_and(|c| is_word(*c)) {
        at
    } else if at > 0 && chars.get(at - 1).is_some_and(|c| is_word(*c)) {
        at - 1
    } else {
        return None;
    };
    let start = (0..=at).rev().take_while(|i| is_word(chars[*i])).last()?;
    let end = (at..chars.len()).take_while(|i| is_word(chars[*i])).last()? + 1;
    let word: String = chars[start..end].iter().collect();
    (word.chars().count() >= 2 && !word.chars().all(|c| c.is_numeric())).then_some(word)
}

/// Char ranges of the whole-word uses of `word` in `text`.
fn uses(text: &str, word: &str) -> Vec<(u32, u32)> {
    let mut found = Vec::new();
    let len = word.chars().count() as u32;
    let mut chars = 0u32;
    let mut last_byte = 0;
    for (byte, _) in text.match_indices(word) {
        chars += text[last_byte..byte].chars().count() as u32;
        last_byte = byte;
        let before = text[..byte].chars().next_back();
        let after = text[byte + word.len()..].chars().next();
        if !before.is_some_and(is_word) && !after.is_some_and(is_word) {
            found.push((chars, chars + len));
            if found.len() == MAX_USES {
                break;
            }
        }
    }
    found
}

fn update(doc: DocumentId) -> Result<(), String> {
    if !ENABLED.get() || editor::current_document() != Some(doc) {
        return Ok(());
    }
    let head = editor::selections(doc)?.first().map_or(0, |r| r.head);
    // The word from a small window around the cursor.
    let from = head.saturating_sub(64);
    let window = editor::text_range(doc, from, head + 64).or_else(|_| {
        let len = editor::text(doc)?.chars().count() as u32;
        editor::text_range(doc, from, (head + 64).min(len))
    })?;
    let word = word_at(&window, (head - from) as usize);
    let revision = editor::revision(doc)?;
    let state = word.clone().map(|w| (doc, revision, w));
    if SHOWN.with_borrow(|shown| *shown == state) {
        return Ok(());
    }
    SHOWN.set(state);
    let Some(word) = word else {
        return decorations::set_highlights(doc, "uses", &[]);
    };
    let text = editor::text(doc)?;
    if text.len() as u32 > MAX_CHARS {
        return decorations::set_highlights(doc, "uses", &[]);
    }
    let scope = SCOPE.with_borrow(|s| s.clone());
    let highlights: Vec<_> = uses(&text, &word)
        .into_iter()
        .map(|(from, to)| highlight(from, to, &scope))
        .collect();
    // A word used once has nothing else to show.
    let highlights = if highlights.len() > 1 { highlights } else { Vec::new() };
    decorations::set_highlights(doc, "uses", &highlights)
}

struct Illuminate;

impl Plugin for Illuminate {
    fn init(config: String) -> Result<Registration, String> {
        if let Some(scope) = config
            .split("\"scope\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
        {
            SCOPE.set(scope.to_string());
        }
        Ok(Registration {
            commands: vec![CommandInfo {
                name: "toggle".into(),
                doc: "Turn highlighting the word's uses off or on".into(),
            }],
            events: vec![EventKind::SelectionChange, EventKind::DocumentChange],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        match name {
            "toggle" => {
                ENABLED.set(!ENABLED.get());
                SHOWN.set(None);
                let doc = editor::current_document().ok_or("no document")?;
                if ENABLED.get() {
                    update(doc)
                } else {
                    decorations::set_highlights(doc, "uses", &[])
                }
            }
            _ => Err(format!("unknown command {name}")),
        }
    }

    fn on_selection_change(doc: DocumentId) {
        let _ = update(doc);
    }
    fn on_document_change(doc: DocumentId) {
        let _ = update(doc);
    }
}

register_plugin!(Illuminate);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn words_and_uses() {
        assert_eq!(word_at("let count = 1;", 5).as_deref(), Some("count"));
        assert_eq!(word_at("let count = 1;", 9).as_deref(), Some("count")); // just after
        assert_eq!(word_at("a = 1", 0), None); // too short
        assert_eq!(word_at("x = 12", 4), None); // a number
        assert_eq!(word_at("é_ok", 0).as_deref(), Some("é_ok"));
        assert_eq!(uses("ab abc ab_ (ab) éab ab", "ab"), [(0, 2), (12, 14), (20, 22)]);
    }
}
