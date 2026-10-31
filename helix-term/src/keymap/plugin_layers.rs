//! Keys bound to plugin callbacks (plugin API 0.5 `keymaps`), composed into
//! the keymap in layers. From first to last:
//!
//! 1. plugin buffer keys (in the plugin's own buffers),
//! 2. the `override` layer (the user granted `allow-key-override`),
//! 3. the user's keys,
//! 4. the `plugin` layer,
//! 5. Pure's defaults,
//! 6. the `fallback` layer and plugins' static `[keys]` (only keys nothing
//!    else binds).
//!
//! Within a layer the binding set last wins. A binding is a `plugin-key`
//! typable command; the editor view calls the plugin for it and, when the
//! plugin passes the keys through, looks them up again without it
//! ([`compose`] with `exclude`).

use super::{KeyTrie, KeyTrieNode, MappableCommand};
use arc_swap::ArcSwapOption;
use helix_event::runtime_local;
use helix_view::{document::Mode, input::KeyEvent, DocumentId};
use parking_lot::Mutex;
use std::{collections::HashMap, sync::Arc, sync::OnceLock};

/// Where a plugin binding sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    Override,
    Plugin,
    Fallback,
}

/// Keys bound to a plugin callback.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginBinding {
    pub plugin: String,
    pub name: String,
    pub description: String,
    pub layer: Layer,
    pub mode: Mode,
    pub keys: Vec<KeyEvent>,
    pub document: Option<DocumentId>,
    pub language: Option<String>,
}

/// The document a key is pressed in.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyScope<'a> {
    /// The plugin owning the buffer.
    pub owner: Option<&'a str>,
    pub document: Option<DocumentId>,
    pub language: Option<&'a str>,
}

runtime_local! {
    /// All plugins' bindings, in the order they were set.
    static BINDINGS: ArcSwapOption<Vec<PluginBinding>> = ArcSwapOption::const_empty();
    /// The last composed keymap: (keymap address, bindings address,
    /// document, language) and the result.
    static COMPOSED: Mutex<Option<(CacheKey, Arc<HashMap<Mode, KeyTrie>>)>> = Mutex::new(None);
}

type CacheKey = (usize, usize, Option<DocumentId>, Option<String>);

/// Replace all plugin bindings.
pub fn set_bindings(bindings: Vec<PluginBinding>) {
    BINDINGS.store((!bindings.is_empty()).then(|| Arc::new(bindings)));
    *COMPOSED.lock() = None;
}

/// Whether any plugin binds keys (one atomic load).
pub fn has_bindings() -> bool {
    BINDINGS.load().is_some()
}

/// All plugin bindings, in the order they were set.
pub fn bindings() -> Arc<Vec<PluginBinding>> {
    BINDINGS.load_full().unwrap_or_default()
}

/// The keymap changed (config reload).
pub fn forget_composed() {
    *COMPOSED.lock() = None;
}

/// The command a binding becomes.
pub fn command(binding: &PluginBinding) -> MappableCommand {
    MappableCommand::Typable {
        name: "plugin-key".into(),
        args: format!("{} {}", binding.plugin, binding.name),
        doc: if binding.description.is_empty() {
            format!("{}: {}", binding.plugin, binding.name)
        } else {
            binding.description.clone()
        },
    }
}

/// `(plugin, name)` if `command` is a plugin binding.
pub fn binding_of(command: &MappableCommand) -> Option<(String, String)> {
    match command {
        MappableCommand::Typable { name, args, .. } if name == "plugin-key" => {
            let (plugin, name) = args.split_once(' ')?;
            Some((plugin.to_string(), name.to_string()))
        }
        _ => None,
    }
}

fn defaults() -> &'static HashMap<Mode, KeyTrie> {
    static DEFAULTS: OnceLock<HashMap<Mode, KeyTrie>> = OnceLock::new();
    DEFAULTS.get_or_init(super::default)
}

/// Whether the user's keymap (`config`, which includes Pure's defaults)
/// binds `keys` or one of its prefixes differently from the defaults.
pub fn user_bound(config: &HashMap<Mode, KeyTrie>, mode: Mode, keys: &[KeyEvent]) -> bool {
    let (Some(config), Some(defaults)) = (config.get(&mode), defaults().get(&mode)) else {
        return false;
    };
    (1..=keys.len()).any(|len| {
        let mine = config.search(&keys[..len]);
        let default = defaults.search(&keys[..len]);
        let leaf = matches!(
            mine,
            Some(KeyTrie::MappableCommand(_) | KeyTrie::Sequence(_))
        );
        mine != default && (leaf || len == keys.len())
    })
}

/// How a binding goes in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Insert {
    /// Replace what is there, even a whole sub-menu.
    Override,
    /// Only where nothing is bound.
    Fill,
}

/// Put `leaf` at `keys` in `trie`; `false` if `how` doesn't allow it.
fn insert(trie: &mut KeyTrie, keys: &[KeyEvent], leaf: KeyTrie, how: Insert) -> bool {
    let Some((last, prefix)) = keys.split_last() else {
        return false;
    };
    let mut node = match trie {
        KeyTrie::Node(node) => node,
        _ => return false,
    };
    for key in prefix {
        let entry = node
            .map
            .entry(*key)
            .or_insert_with(|| KeyTrie::Node(KeyTrieNode::new("", Default::default())));
        if !matches!(entry, KeyTrie::Node(_)) {
            if how == Insert::Fill {
                return false;
            }
            *entry = KeyTrie::Node(KeyTrieNode::new("", Default::default()));
        }
        node = match entry {
            KeyTrie::Node(node) => node,
            _ => unreachable!(),
        };
    }
    match (node.map.get(last), how) {
        (Some(_), Insert::Fill) => false,
        _ => {
            node.map.insert(*last, leaf);
            true
        }
    }
}

/// Whether `binding` applies in `scope`.
fn applies(binding: &PluginBinding, scope: &KeyScope) -> bool {
    binding
        .document
        .is_none_or(|doc| scope.document == Some(doc))
        && binding
            .language
            .as_deref()
            .is_none_or(|language| scope.language == Some(language))
}

/// `keymap` (the user's keys over Pure's, with plugins' static keys
/// filled in) with the plugin bindings that apply in `scope`, without
/// `exclude` (`(plugin, name)`). `config` is the user's keymap without
/// plugin keys. `None`: there are no bindings.
pub fn compose(
    config: &HashMap<Mode, KeyTrie>,
    keymap: &HashMap<Mode, KeyTrie>,
    scope: &KeyScope,
    exclude: Option<(&str, &str)>,
) -> Option<Arc<HashMap<Mode, KeyTrie>>> {
    let bindings = BINDINGS.load_full()?;
    // Only scopes some binding is about make a different keymap.
    let document = scope
        .document
        .filter(|doc| bindings.iter().any(|b| b.document == Some(*doc)));
    let language = scope
        .language
        .filter(|language| {
            bindings
                .iter()
                .any(|b| b.language.as_deref() == Some(*language))
        })
        .map(str::to_string);
    let key = (
        keymap as *const _ as usize,
        Arc::as_ptr(&bindings) as usize,
        document,
        language,
    );
    if exclude.is_none() {
        if let Some((cached, map)) = &*COMPOSED.lock() {
            if *cached == key {
                return Some(map.clone());
            }
        }
    }
    let mut map = keymap.clone();
    let active = bindings.iter().filter(|binding| {
        applies(binding, scope)
            && exclude
                .is_none_or(|(plugin, name)| (plugin, name) != (&binding.plugin, &binding.name))
    });
    let mut layers: Vec<_> = active.collect();
    // Lowest layer first, so higher ones go over it; within a layer, in
    // the order they were set (the last wins).
    layers.sort_by_key(|binding| match binding.layer {
        Layer::Fallback => 0,
        Layer::Plugin => 1,
        Layer::Override => 2,
    });
    for binding in layers {
        let Some(trie) = map.get_mut(&binding.mode) else {
            continue;
        };
        let leaf = KeyTrie::MappableCommand(command(binding));
        match binding.layer {
            Layer::Fallback => {
                insert(trie, &binding.keys, leaf, Insert::Fill);
            }
            // Above Pure's keys, menus included (`g`, `space`...), unless
            // the user bound something there.
            Layer::Plugin if !user_bound(config, binding.mode, &binding.keys) => {
                insert(trie, &binding.keys, leaf, Insert::Override);
            }
            Layer::Plugin => {}
            Layer::Override => {
                insert(trie, &binding.keys, leaf, Insert::Override);
            }
        }
    }
    let map = Arc::new(map);
    if exclude.is_none() {
        *COMPOSED.lock() = Some((key, map.clone()));
    }
    Some(map)
}

/// What shadows what, for `:plugin-keys`: one line per binding.
pub fn report(config: &HashMap<Mode, KeyTrie>, keymap: &HashMap<Mode, KeyTrie>) -> Vec<String> {
    let bindings = bindings();
    let mut lines = Vec::new();
    for (index, binding) in bindings.iter().enumerate() {
        let keys: Vec<String> = binding.keys.iter().map(|key| key.to_string()).collect();
        let keys = keys.join(" ");
        let mut line = format!(
            "{:<8} {:<16} {:<9} {}/{}",
            format!("{:?}", binding.mode).to_lowercase(),
            keys,
            format!("{:?}", binding.layer).to_lowercase(),
            binding.plugin,
            binding.name
        );
        if let Some(language) = &binding.language {
            line.push_str(&format!(" (in {language} documents)"));
        }
        if binding.document.is_some() {
            line.push_str(" (in one document)");
        }
        let existing = keymap
            .get(&binding.mode)
            .and_then(|trie| trie.search(&binding.keys))
            .map(|trie| match trie {
                KeyTrie::MappableCommand(command) => command.name().to_string(),
                KeyTrie::Sequence(_) => "a command sequence".to_string(),
                KeyTrie::Node(_) => "a key menu".to_string(),
            });
        let user = user_bound(config, binding.mode, &binding.keys);
        let note = match (binding.layer, existing) {
            (_, None) => None,
            (Layer::Fallback, Some(what)) => Some(format!("inactive: {what} is bound there")),
            (Layer::Plugin, Some(what)) if user => {
                Some(format!("inactive: your keys bind {what} there"))
            }
            (Layer::Plugin, Some(what)) if what == "a key menu" => {
                Some("inactive: a key menu is there".to_string())
            }
            (Layer::Plugin, Some(what)) => Some(format!("replaces Pure's {what}")),
            (Layer::Override, Some(what)) if user => Some(format!("replaces your {what}")),
            (Layer::Override, Some(what)) => Some(format!("replaces Pure's {what}")),
        };
        if let Some(note) = note {
            line.push_str(&format!(": {note}"));
        }
        // A later binding of the same keys, mode and scope in the same layer
        // wins.
        if let Some(later) = bindings[index + 1..].iter().find(|other| {
            other.mode == binding.mode
                && other.keys == binding.keys
                && other.layer == binding.layer
                && other.document == binding.document
                && other.language == binding.language
        }) {
            line.push_str(&format!(" [shadowed by {}/{}]", later.plugin, later.name));
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::keymap::macros::keymap;
    use helix_core::hashmap;
    use std::str::FromStr;

    /// The bindings are global: one test at a time.
    static LOCK: Mutex<()> = Mutex::new(());

    fn keys(keys: &str) -> Vec<KeyEvent> {
        keys.split(' ')
            .map(|key| KeyEvent::from_str(key).unwrap())
            .collect()
    }

    fn binding(name: &str, keys_: &str, layer: Layer) -> PluginBinding {
        PluginBinding {
            plugin: "p".into(),
            name: name.into(),
            description: String::new(),
            layer,
            mode: Mode::Normal,
            keys: keys(keys_),
            document: None,
            language: None,
        }
    }

    fn lookup(map: &HashMap<Mode, KeyTrie>, keys_: &str) -> Option<String> {
        map[&Mode::Normal]
            .search(&keys(keys_))
            .map(|trie| match trie {
                KeyTrie::MappableCommand(command) => match binding_of(command) {
                    Some((_, name)) => format!("plugin:{name}"),
                    None => command.name().to_string(),
                },
                KeyTrie::Sequence(_) => "sequence".into(),
                KeyTrie::Node(_) => "node".into(),
            })
    }

    #[test]
    fn layers_follow_the_documented_precedence() {
        let _lock = LOCK.lock();
        // The user binds `C-k` and adds to the `space` menu; Pure binds `w`
        // and the `g`, `z` and `space` menus.
        let mut config = crate::keymap::default();
        crate::keymap::merge_keys(
            &mut config,
            hashmap! { Mode::Normal => keymap!({ "Normal"
                "C-k" => goto_file_start,
                "space" => { "Space" "X" => goto_file_end, },
            }) },
        );
        set_bindings(vec![
            binding("over-user", "C-k", Layer::Plugin),
            binding("over-native", "w", Layer::Plugin),
            binding("over-menu", "g", Layer::Plugin),
            binding("over-user-menu", "space", Layer::Plugin),
            binding("fill-taken", "b", Layer::Fallback),
            binding("fill-free", "C-q", Layer::Fallback),
            binding("override", "e", Layer::Override),
            binding("deep", "z q x", Layer::Plugin),
        ]);
        let map = compose(&config, &config, &KeyScope::default(), None).unwrap();
        // The user's keys beat the plugin layer; the plugin layer beats Pure's.
        assert_eq!(lookup(&map, "C-k").as_deref(), Some("goto_file_start"));
        assert_eq!(lookup(&map, "w").as_deref(), Some("plugin:over-native"));
        // Pure's whole menus too...
        assert_eq!(lookup(&map, "g").as_deref(), Some("plugin:over-menu"));
        // ...but not one the user added to.
        assert_eq!(lookup(&map, "space").as_deref(), Some("node"));
        assert_eq!(lookup(&map, "space X").as_deref(), Some("goto_file_end"));
        // Deeper keys go into a menu, which keeps its other keys.
        assert_eq!(lookup(&map, "z q x").as_deref(), Some("plugin:deep"));
        assert_eq!(lookup(&map, "z z").as_deref(), Some("align_view_center"));
        // The fallback layer only takes free keys.
        assert_eq!(lookup(&map, "b").as_deref(), Some("move_prev_word_start"));
        assert_eq!(lookup(&map, "C-q").as_deref(), Some("plugin:fill-free"));
        assert_eq!(lookup(&map, "e").as_deref(), Some("plugin:override"));
        // Passing through: the same keys without the binding.
        let without = compose(
            &config,
            &config,
            &KeyScope::default(),
            Some(("p", "over-native")),
        )
        .unwrap();
        assert_eq!(
            lookup(&without, "w").as_deref(),
            Some("move_next_word_start")
        );
        let report = report(&config, &config);
        assert!(report.iter().any(|line| line.contains("p/over-user")
            && line.contains("inactive: your keys bind goto_file_start there")));
        assert!(report
            .iter()
            .any(|line| line.contains("p/over-native") && line.contains("replaces Pure's")));
        set_bindings(Vec::new());
    }

    #[test]
    fn the_last_binding_wins_and_scopes_apply() {
        let _lock = LOCK.lock();
        let config = crate::keymap::default();
        let mut first = binding("first", "C-q", Layer::Plugin);
        first.plugin = "a".into();
        let second = binding("second", "C-q", Layer::Plugin);
        let mut rust_only = binding("rust", "C-y", Layer::Plugin);
        rust_only.language = Some("rust".into());
        set_bindings(vec![first, second, rust_only]);
        let map = compose(&config, &config, &KeyScope::default(), None).unwrap();
        assert_eq!(lookup(&map, "C-q").as_deref(), Some("plugin:second"));
        assert_eq!(lookup(&map, "C-y"), None);
        let rust = KeyScope {
            language: Some("rust"),
            ..Default::default()
        };
        let map = compose(&config, &config, &rust, None).unwrap();
        assert_eq!(lookup(&map, "C-y").as_deref(), Some("plugin:rust"));
        let report = report(&config, &config);
        assert!(report
            .iter()
            .any(|line| line.contains("a/first") && line.contains("[shadowed by p/second]")));
        set_bindings(Vec::new());
    }
}
