//! Example plugin: snippets (API 0.4 `snippets.complete`). Type a snippet's
//! trigger (`fn`, `for`, `test`...) and the completion menu offers it;
//! accepting it inserts the snippet with its tab stops: the first
//! placeholder is selected, tab goes to the next one, `$0` is where the
//! cursor ends. The editor's own snippet engine does this, as for a language
//! server's snippets.
//!
//! Own snippets, by language (`*` for all), in `plugins.toml`:
//!
//! ```toml
//! [snippets.snippets.rust]
//! dbg = "dbg!(${1:value});"
//! [snippets.snippets."*"]
//! todo = "TODO(${1:me}): $0"
//! ```

use std::cell::RefCell;
use std::collections::BTreeMap;

use helix_plugin_api::{
    completion::{self, item, CompletionItem},
    editor, register_plugin, DocumentId, EventKind, Plugin, Registration,
};

/// Built-in snippets: `(language, trigger, snippet)`; `*` is any language.
const BUILT_IN: &[(&str, &str, &str)] = &[
    ("rust", "fn", "fn ${1:name}(${2}) {\n    $0\n}"),
    ("rust", "test", "#[test]\nfn ${1:name}() {\n    $0\n}"),
    ("rust", "impl", "impl ${1:Type} {\n    $0\n}"),
    ("rust", "match", "match ${1:value} {\n    ${2:pattern} => $0,\n}"),
    ("rust", "for", "for ${1:item} in ${2:items} {\n    $0\n}"),
    ("python", "def", "def ${1:name}(${2}):\n    ${0:pass}"),
    ("python", "for", "for ${1:item} in ${2:items}:\n    ${0:pass}"),
    ("python", "main", "if __name__ == \"__main__\":\n    ${0:main()}"),
    ("javascript", "fn", "function ${1:name}(${2}) {\n  $0\n}"),
    ("javascript", "for", "for (const ${1:item} of ${2:items}) {\n  $0\n}"),
    ("typescript", "fn", "function ${1:name}(${2}): ${3:void} {\n  $0\n}"),
    ("go", "fn", "func ${1:name}(${2}) {\n\t$0\n}"),
    ("go", "iferr", "if err != nil {\n\treturn ${1:err}\n}"),
    ("markdown", "link", "[${1:text}](${2:url})$0"),
    ("markdown", "code", "```${1:language}\n$0\n```"),
    ("*", "todo", "TODO: $0"),
];

thread_local! {
    /// The user's snippets: language → trigger → snippet.
    static OWN: RefCell<BTreeMap<String, BTreeMap<String, String>>> = RefCell::new(BTreeMap::new());
}

/// The user's snippets from the plugin's settings (`{"snippets": {...}}`).
fn parse_settings(config: &str) -> Result<BTreeMap<String, BTreeMap<String, String>>, String> {
    let value: serde_json::Value =
        serde_json::from_str(config).map_err(|err| format!("invalid settings: {err}"))?;
    let mut own = BTreeMap::new();
    let Some(languages) = value.get("snippets").and_then(|s| s.as_object()) else {
        return Ok(own);
    };
    for (language, snippets) in languages {
        let snippets = snippets
            .as_object()
            .ok_or_else(|| format!("snippets.{language} must be a table"))?;
        let mut table = BTreeMap::new();
        for (trigger, snippet) in snippets {
            let snippet = snippet
                .as_str()
                .ok_or_else(|| format!("snippets.{language}.{trigger} must be a string"))?;
            table.insert(trigger.clone(), snippet.to_string());
        }
        own.insert(language.clone(), table);
    }
    Ok(own)
}

/// Snippets for `language` whose trigger starts with `typed`: the user's
/// first (they replace built-in ones with the same trigger).
fn matching(
    own: &BTreeMap<String, BTreeMap<String, String>>,
    language: Option<&str>,
    typed: &str,
) -> Vec<(String, String)> {
    let applies = |lang: &str| lang == "*" || Some(lang) == language;
    let mut found: Vec<(String, String)> = Vec::new();
    for (lang, snippets) in own {
        if applies(lang) {
            for (trigger, snippet) in snippets {
                if trigger.starts_with(typed) && !found.iter().any(|(t, _)| t == trigger) {
                    found.push((trigger.clone(), snippet.clone()));
                }
            }
        }
    }
    for (lang, trigger, snippet) in BUILT_IN {
        if applies(lang) && trigger.starts_with(typed) && !found.iter().any(|(t, _)| t == trigger) {
            found.push((trigger.to_string(), snippet.to_string()));
        }
    }
    found
}

/// The word before the cursor at the end of `line`.
fn word_before(line: &str) -> &str {
    let start = line
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map_or(line.len(), |(i, _)| i);
    &line[start..]
}

fn items(found: Vec<(String, String)>) -> Vec<CompletionItem> {
    found
        .into_iter()
        .map(|(trigger, snippet)| {
            let mut item = item(trigger);
            item.documentation = Some(format!("```\n{snippet}\n```"));
            item.insert = Some(snippet);
            item.kind = Some("snippet".into());
            item
        })
        .collect()
}

struct Snippets;

impl Plugin for Snippets {
    fn init(config: String) -> Result<Registration, String> {
        let own = parse_settings(&config)?;
        OWN.set(own);
        Ok(Registration {
            commands: Vec::new(),
            events: vec![EventKind::Completion],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        Err(format!("unknown command {name}"))
    }

    fn on_completion(id: u32, doc: DocumentId, position: u32) {
        let start = position.saturating_sub(40);
        let typed = editor::text_range(doc, start, position)
            .ok()
            .map(|before| word_before(before.rsplit('\n').next().unwrap_or("")).to_string())
            .unwrap_or_default();
        let found = if typed.is_empty() {
            Vec::new()
        } else {
            let language = editor::document_language(doc).ok().flatten();
            OWN.with_borrow(|own| matching(own, language.as_deref(), &typed))
        };
        let result = if found.is_empty() {
            completion::complete(id, &[])
        } else {
            completion::complete_snippets(id, &items(found))
        };
        if let Err(err) = result {
            editor::log(&err);
        }
    }
}

register_plugin!(Snippets);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn own_snippets_come_first_and_replace_built_in_ones() {
        let own = parse_settings(
            r#"{"snippets": {"rust": {"fn": "fn ${1:x}() {}", "dbg": "dbg!($1)"}, "*": {"todo": "TODO: $0"}}}"#,
        )
        .unwrap();
        let found = matching(&own, Some("rust"), "f");
        assert_eq!(found[0], ("fn".to_string(), "fn ${1:x}() {}".to_string()));
        assert!(found.iter().any(|(t, _)| t == "for"));
        assert_eq!(found.iter().filter(|(t, _)| t == "fn").count(), 1);
        assert_eq!(matching(&own, Some("python"), "to")[0].0, "todo");
        assert!(matching(&own, Some("python"), "fn").is_empty());
        assert!(parse_settings(r#"{"snippets": {"rust": {"x": 1}}}"#).is_err());
    }

    #[test]
    fn the_word_before_the_cursor() {
        assert_eq!(word_before("    let x = fo"), "fo");
        assert_eq!(word_before("x."), "");
        assert_eq!(word_before("é_1"), "é_1");
    }
}
