pub mod default;
pub mod macros;
pub mod modes;
pub mod plugin_layers;

pub use crate::commands::MappableCommand;
pub use default::default;

use arc_swap::{
    access::{DynAccess, DynGuard},
    ArcSwap, ArcSwapOption,
};
use helix_event::runtime_local;
use helix_view::{document::Mode, info::Info, input::KeyEvent};
use indexmap::IndexMap;
use macros::key;
use parking_lot::Mutex;
use serde::Deserialize;
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
    ops::{Deref, DerefMut},
    sync::Arc,
};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct KeyTrieNode {
    /// A label for keys coming under this node, like "Goto mode"
    #[serde(skip)]
    name: String,
    #[serde(flatten)]
    map: IndexMap<KeyEvent, KeyTrie>,
    #[serde(skip)]
    pub is_sticky: bool,
}

impl KeyTrieNode {
    pub fn new(name: &str, map: IndexMap<KeyEvent, KeyTrie>) -> Self {
        Self {
            name: name.to_string(),
            map,
            is_sticky: false,
        }
    }

    /// Merge another Node in. Leaves and subnodes from the other node replace
    /// corresponding keyevent in self, except when both other and self have
    /// subnodes for same key. In that case the merge is recursive.
    pub fn merge(&mut self, mut other: Self) {
        for (key, trie) in std::mem::take(&mut other.map) {
            if let Some(KeyTrie::Node(node)) = self.map.get_mut(&key) {
                if let KeyTrie::Node(other_node) = trie {
                    node.merge(other_node);
                    continue;
                }
            }
            self.map.insert(key, trie);
        }
    }

    /// Add `other`'s keys where `self` binds nothing: unlike `merge`, existing
    /// bindings always win (for plugins' default bindings).
    pub fn fill(&mut self, mut other: Self) {
        for (key, trie) in std::mem::take(&mut other.map) {
            match (self.map.get_mut(&key), trie) {
                (None, trie) => {
                    self.map.insert(key, trie);
                }
                (Some(KeyTrie::Node(node)), KeyTrie::Node(other)) => node.fill(other),
                _ => {}
            }
        }
    }

    pub fn infobox(&self) -> Info {
        let mut body: Vec<(BTreeSet<KeyEvent>, &str)> = Vec::with_capacity(self.len());
        for (&key, trie) in self.iter() {
            let desc = match trie {
                KeyTrie::MappableCommand(cmd) => {
                    if cmd.name() == "no_op" {
                        continue;
                    }
                    cmd.doc()
                }
                KeyTrie::Node(n) => &n.name,
                KeyTrie::Sequence(_) => "[Multiple commands]",
            };
            match body.iter().position(|(_, d)| d == &desc) {
                Some(pos) => {
                    body[pos].0.insert(key);
                }
                None => body.push((BTreeSet::from([key]), desc)),
            }
        }

        let body: Vec<_> = body
            .into_iter()
            .map(|(events, desc)| {
                let events = events.iter().map(ToString::to_string).collect::<Vec<_>>();
                (events.join(", "), desc)
            })
            .collect();
        Info::new(self.name.clone(), &body)
    }
}

impl PartialEq for KeyTrieNode {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl Deref for KeyTrieNode {
    type Target = IndexMap<KeyEvent, KeyTrie>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl DerefMut for KeyTrieNode {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.map
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeyTrie {
    MappableCommand(MappableCommand),
    Sequence(Vec<MappableCommand>),
    Node(KeyTrieNode),
}

impl<'de> Deserialize<'de> for KeyTrie {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(KeyTrieVisitor)
    }
}

struct KeyTrieVisitor;

impl<'de> serde::de::Visitor<'de> for KeyTrieVisitor {
    type Value = KeyTrie;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "a command, list of commands, or sub-keymap")
    }

    fn visit_str<E>(self, command: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        command
            .parse::<MappableCommand>()
            .map(KeyTrie::MappableCommand)
            .map_err(E::custom)
    }

    fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
    where
        S: serde::de::SeqAccess<'de>,
    {
        let mut commands = Vec::new();
        while let Some(command) = seq.next_element::<String>()? {
            commands.push(
                command
                    .parse::<MappableCommand>()
                    .map_err(serde::de::Error::custom)?,
            )
        }

        // A key macro in a sequence gives its keys to the command before it
        // that waits for one (`["select_textobject_inner", "@w"]`, see
        // `commands::run_sequence`); it can't start one.
        if matches!(commands.first(), Some(MappableCommand::Macro { .. })) {
            return Err(serde::de::Error::custom(
                "a key macro in a command sequence must follow a command that waits for a key",
            ));
        }

        Ok(KeyTrie::Sequence(commands))
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: serde::de::MapAccess<'de>,
    {
        let mut mapping = IndexMap::new();
        while let Some((key, value)) = map.next_entry::<KeyEvent, KeyTrie>()? {
            mapping.insert(key, value);
        }
        Ok(KeyTrie::Node(KeyTrieNode::new("", mapping)))
    }
}

impl KeyTrie {
    pub fn reverse_map(&self) -> ReverseKeymap {
        // recursively visit all nodes in keymap
        fn map_node(cmd_map: &mut ReverseKeymap, node: &KeyTrie, keys: &mut Vec<KeyEvent>) {
            match node {
                KeyTrie::MappableCommand(MappableCommand::Macro { .. }) => {}
                KeyTrie::MappableCommand(cmd) => {
                    let name = cmd.name();
                    if name != "no_op" {
                        cmd_map.entry(name.into()).or_default().push(keys.clone())
                    }
                }
                KeyTrie::Node(next) => {
                    for (key, trie) in &next.map {
                        keys.push(*key);
                        map_node(cmd_map, trie, keys);
                        keys.pop();
                    }
                }
                KeyTrie::Sequence(_) => {}
            };
        }

        let mut res = HashMap::new();
        map_node(&mut res, self, &mut Vec::new());
        res
    }

    pub fn node(&self) -> Option<&KeyTrieNode> {
        match *self {
            KeyTrie::Node(ref node) => Some(node),
            KeyTrie::MappableCommand(_) | KeyTrie::Sequence(_) => None,
        }
    }

    pub fn node_mut(&mut self) -> Option<&mut KeyTrieNode> {
        match *self {
            KeyTrie::Node(ref mut node) => Some(node),
            KeyTrie::MappableCommand(_) | KeyTrie::Sequence(_) => None,
        }
    }

    /// Merge another KeyTrie in, assuming that this KeyTrie and the other
    /// are both Nodes. Panics otherwise.
    pub fn merge_nodes(&mut self, mut other: Self) {
        let node = std::mem::take(other.node_mut().unwrap());
        self.node_mut().unwrap().merge(node);
    }

    /// Descend a trie following the given path of keys
    pub fn search(&self, keys: &[KeyEvent]) -> Option<&KeyTrie> {
        let mut trie = self;
        for key in keys {
            trie = match trie {
                KeyTrie::Node(map) => map.get(key),
                // leaf encountered while keys left to process
                KeyTrie::MappableCommand(_) | KeyTrie::Sequence(_) => None,
            }?
        }
        Some(trie)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeymapResult {
    /// Needs more keys to execute a command. Contains valid keys for next keystroke.
    Pending(KeyTrieNode),
    Matched(MappableCommand),
    /// Matched a sequence of commands to execute.
    MatchedSequence(Vec<MappableCommand>),
    /// Key was not found in the root keymap
    NotFound,
    /// Key is invalid in combination with previous keys. Contains keys leading upto
    /// and including current (invalid) key.
    Cancelled(Vec<KeyEvent>),
}

/// A map of command names to keybinds that will execute the command.
pub type ReverseKeymap = HashMap<String, Vec<Vec<KeyEvent>>>;

runtime_local! {
    /// Plugins' default bindings, below the user's and Pure's (see
    /// [`set_plugin_keys`]).
    static PLUGIN_KEYS: ArcSwapOption<HashMap<Mode, KeyTrie>> = ArcSwapOption::const_empty();
    /// The keymap with plugin keys filled in, for the keymap (by address)
    /// and plugin keys it was made from.
    static FILLED: Mutex<Option<(usize, usize, Arc<HashMap<Mode, KeyTrie>>)>> = Mutex::new(None);
    /// Bindings plugins set for their own buffers, by plugin (see
    /// [`set_buffer_keys`]).
    static BUFFER_KEYS: ArcSwapOption<HashMap<String, HashMap<Mode, KeyTrie>>> = ArcSwapOption::const_empty();
    /// The last keymap with a plugin's buffer keys on top: `(keymap address,
    /// buffer keys address, plugin, keymap)`.
    static IN_BUFFER: Mutex<Option<(usize, usize, String, Arc<HashMap<Mode, KeyTrie>>)>> = Mutex::new(None);
}

/// Set the bindings plugins use in their own buffers (`open-buffer`). There
/// they take precedence over every other binding; elsewhere they do nothing.
pub fn set_buffer_keys(keys: Option<HashMap<String, HashMap<Mode, KeyTrie>>>) {
    BUFFER_KEYS.store(keys.map(Arc::new));
    *IN_BUFFER.lock() = None;
}

/// `keymaps` with `owner`'s buffer keys on top, if it has any.
fn with_buffer_keys(
    keymaps: &HashMap<Mode, KeyTrie>,
    owner: &str,
) -> Option<Arc<HashMap<Mode, KeyTrie>>> {
    let all = BUFFER_KEYS.load_full()?;
    let keys = all.get(owner)?;
    let base = keymaps as *const _ as usize;
    let all_ptr = Arc::as_ptr(&all) as usize;
    let mut cached = IN_BUFFER.lock();
    if let Some((b, k, plugin, map)) = &*cached {
        if *b == base && *k == all_ptr && plugin == owner {
            return Some(map.clone());
        }
    }
    let mut map = keymaps.clone();
    for (mode, trie) in keys {
        if let (Some(KeyTrie::Node(node)), KeyTrie::Node(extra)) = (map.get_mut(mode), trie) {
            node.merge(extra.clone());
        }
    }
    let map = Arc::new(map);
    *cached = Some((base, all_ptr, owner.to_string(), map.clone()));
    Some(map)
}

/// Set the plugins' default bindings. They only bind keys that neither the
/// user's config nor the default keymap bind.
pub fn set_plugin_keys(keys: Option<HashMap<Mode, KeyTrie>>) {
    PLUGIN_KEYS.store(keys.map(Arc::new));
    *FILLED.lock() = None;
}

/// The keymap may have changed (config reload): fill plugin keys again.
pub fn forget_filled_keymap() {
    *FILLED.lock() = None;
    *IN_BUFFER.lock() = None;
    plugin_layers::forget_composed();
    modes::forget_composed();
}

/// `keymaps` with the plugin keys filled in, or `None` without plugin keys
/// (the usual case: one atomic load).
fn with_plugin_keys(keymaps: &HashMap<Mode, KeyTrie>) -> Option<Arc<HashMap<Mode, KeyTrie>>> {
    let plugin = PLUGIN_KEYS.load_full()?;
    let base = keymaps as *const _ as usize;
    let plugin_ptr = Arc::as_ptr(&plugin) as usize;
    let mut filled = FILLED.lock();
    if let Some((b, p, map)) = &*filled {
        if *b == base && *p == plugin_ptr {
            return Some(map.clone());
        }
    }
    let mut map = keymaps.clone();
    for (mode, trie) in plugin.iter() {
        if let (Some(KeyTrie::Node(node)), KeyTrie::Node(extra)) = (map.get_mut(mode), trie) {
            node.fill(extra.clone());
        }
    }
    let map = Arc::new(map);
    *filled = Some((base, plugin_ptr, map.clone()));
    Some(map)
}

pub struct Keymaps {
    pub map: Box<dyn DynAccess<HashMap<Mode, KeyTrie>>>,
    /// Stores pending keys waiting for the next key. This is relative to a
    /// sticky node if one is in use.
    state: Vec<KeyEvent>,
    /// Stores the sticky node if one is activated.
    pub sticky: Option<KeyTrieNode>,
}

impl Keymaps {
    pub fn new(map: Box<dyn DynAccess<HashMap<Mode, KeyTrie>>>) -> Self {
        Self {
            map,
            state: Vec::new(),
            sticky: None,
        }
    }

    pub fn map(&self) -> DynGuard<HashMap<Mode, KeyTrie>> {
        self.map.load()
    }

    /// Returns list of keys waiting to be disambiguated in current mode.
    pub fn pending(&self) -> &[KeyEvent] {
        &self.state
    }

    pub fn sticky(&self) -> Option<&KeyTrieNode> {
        self.sticky.as_ref()
    }

    pub fn contains_key(&self, mode: Mode, key: KeyEvent) -> bool {
        // Plugin keys count too (a plugin may bind `space 1`).
        let base = self.map();
        let filled = with_plugin_keys(&base);
        let keymaps = filled.as_deref().unwrap_or(&base);
        let layered = plugin_layers::compose(&base, keymaps, &Default::default(), None);
        let keymaps = layered.as_deref().unwrap_or(keymaps);
        let keymap = &keymaps[&mode];
        keymap
            .search(self.pending())
            .and_then(KeyTrie::node)
            .is_some_and(|node| node.contains_key(&key))
    }

    /// Lookup `key` in the keymap to try and find a command to execute. Escape
    /// key cancels pending keystrokes. If there are no pending keystrokes but a
    /// sticky node is in use, it will be cleared.
    pub fn get(&mut self, mode: Mode, key: KeyEvent) -> KeymapResult {
        self.get_in(mode, key, None)
    }

    /// [`Self::get`] in a buffer of plugin `owner`, whose buffer keys apply.
    pub fn get_in(&mut self, mode: Mode, key: KeyEvent, owner: Option<&str>) -> KeymapResult {
        let scope = plugin_layers::KeyScope {
            owner,
            ..Default::default()
        };
        self.get_scoped(mode, key, &scope)
    }

    /// [`Self::get`] in the document of `scope`: plugin bindings for that
    /// document or its language, and its owner's buffer keys, apply.
    pub fn get_scoped(
        &mut self,
        mode: Mode,
        key: KeyEvent,
        scope: &plugin_layers::KeyScope,
    ) -> KeymapResult {
        // TODO: remove the sticky part and look up manually
        let base = self.map();
        let filled = with_plugin_keys(&base);
        let keymaps = filled.as_deref().unwrap_or(&base);
        let layered = plugin_layers::compose(&base, keymaps, scope, None);
        let keymaps = layered.as_deref().unwrap_or(keymaps);
        // Key modes on in the document go over the rest (not a plugin's
        // own buffer keys).
        let moded = modes::compose(keymaps, scope.document, scope.language);
        let keymaps = moded.as_deref().unwrap_or(keymaps);
        let in_buffer = scope
            .owner
            .and_then(|owner| with_buffer_keys(keymaps, owner));
        let keymaps = in_buffer.as_deref().unwrap_or(keymaps);
        let keymap = &keymaps[&mode];

        if key!(Esc) == key {
            if !self.state.is_empty() {
                // Note that Esc is not included here
                return KeymapResult::Cancelled(self.state.drain(..).collect());
            }
            self.sticky = None;
        }

        let first = self.state.first().unwrap_or(&key);
        let trie_node = match self.sticky {
            Some(ref trie) => Cow::Owned(KeyTrie::Node(trie.clone())),
            None => Cow::Borrowed(keymap),
        };

        let trie = match trie_node.search(&[*first]) {
            Some(KeyTrie::MappableCommand(ref cmd)) => {
                return KeymapResult::Matched(cmd.clone());
            }
            Some(KeyTrie::Sequence(ref cmds)) => {
                return KeymapResult::MatchedSequence(cmds.clone());
            }
            None => return KeymapResult::NotFound,
            Some(t) => t,
        };

        self.state.push(key);
        match trie.search(&self.state[1..]) {
            Some(KeyTrie::Node(map)) => {
                if map.is_sticky {
                    self.state.clear();
                    self.sticky = Some(map.clone());
                }
                KeymapResult::Pending(map.clone())
            }
            Some(KeyTrie::MappableCommand(cmd)) => {
                self.state.clear();
                KeymapResult::Matched(cmd.clone())
            }
            Some(KeyTrie::Sequence(cmds)) => {
                self.state.clear();
                KeymapResult::MatchedSequence(cmds.clone())
            }
            None => KeymapResult::Cancelled(self.state.drain(..).collect()),
        }
    }
}

impl Keymaps {
    /// What `keys` would do in `mode` and `scope` without plugin binding
    /// `exclude` (`(plugin, name)`): for a binding that passes its keys
    /// through.
    pub fn without_binding(
        &self,
        mode: Mode,
        keys: &[KeyEvent],
        scope: &plugin_layers::KeyScope,
        exclude: (&str, &str),
    ) -> Option<KeyTrie> {
        let base = self.map();
        let filled = with_plugin_keys(&base);
        let keymaps = filled.as_deref().unwrap_or(&base);
        let layered = plugin_layers::compose(&base, keymaps, scope, Some(exclude));
        let keymaps = layered.as_deref().unwrap_or(keymaps);
        let moded = modes::compose(keymaps, scope.document, scope.language);
        let keymaps = moded.as_deref().unwrap_or(keymaps);
        let in_buffer = scope
            .owner
            .and_then(|owner| with_buffer_keys(keymaps, owner));
        let keymaps = in_buffer.as_deref().unwrap_or(keymaps);
        keymaps.get(&mode)?.search(keys).cloned()
    }

    /// Whether `key` (after the keys pending) runs a plugin binding in
    /// `mode` and `scope`, without consuming anything.
    pub fn is_plugin_binding(
        &self,
        mode: Mode,
        key: KeyEvent,
        scope: &plugin_layers::KeyScope,
    ) -> bool {
        if !plugin_layers::has_bindings() {
            return false;
        }
        let base = self.map();
        let filled = with_plugin_keys(&base);
        let keymaps = filled.as_deref().unwrap_or(&base);
        let layered = plugin_layers::compose(&base, keymaps, scope, None);
        let keymaps = layered.as_deref().unwrap_or(keymaps);
        let moded = modes::compose(keymaps, scope.document, scope.language);
        let keymaps = moded.as_deref().unwrap_or(keymaps);
        let mut keys = self.state.clone();
        keys.push(key);
        let trie = match &self.sticky {
            Some(node) => KeyTrie::Node(node.clone()),
            None => match keymaps.get(&mode) {
                Some(trie) => trie.clone(),
                None => return false,
            },
        };
        matches!(
            trie.search(&keys),
            Some(KeyTrie::MappableCommand(command)) if plugin_layers::binding_of(command).is_some()
        )
    }

    /// `:plugin-keys`: every plugin binding with its layer and conflicts.
    pub fn plugin_report(&self) -> Vec<String> {
        let base = self.map();
        let filled = with_plugin_keys(&base);
        let keymaps = filled.as_deref().unwrap_or(&base);
        plugin_layers::report(&base, keymaps)
    }
}

impl Default for Keymaps {
    fn default() -> Self {
        Self::new(Box::new(ArcSwap::new(Arc::new(default()))))
    }
}

/// Merge default config keys with user overwritten keys for custom user config.
pub fn merge_keys(dst: &mut HashMap<Mode, KeyTrie>, mut delta: HashMap<Mode, KeyTrie>) {
    for (mode, keys) in dst {
        keys.merge_nodes(
            delta
                .remove(mode)
                .unwrap_or_else(|| KeyTrie::Node(KeyTrieNode::default())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::macros::keymap;
    use super::*;
    use crate::commands::MappableCommand;
    use arc_swap::access::Constant;
    use helix_core::hashmap;
    use helix_view::input::{KeyCode, KeyEvent, KeyModifiers};
    use indexmap::indexmap;

    #[test]
    #[should_panic]
    fn duplicate_keys_should_panic() {
        keymap!({ "Normal mode"
            "i" => normal_mode,
            "i" => goto_definition,
        });
    }

    #[test]
    fn check_duplicate_keys_in_default_keymap() {
        // will panic on duplicate keys, assumes that `Keymaps` uses keymap! macro
        Keymaps::default();
    }

    #[test]
    fn merge_partial_keys() {
        let keymap = hashmap! {
            Mode::Normal => keymap!({ "Normal mode"
                "i" => normal_mode,
                "无" => insert_mode,
                "z" => jump_backward,
                "g" => { "Merge into goto mode"
                    "$" => goto_line_end,
                    "g" => delete_char_forward,
                },
            })
        };
        let mut merged_keyamp = default();
        merge_keys(&mut merged_keyamp, keymap.clone());
        assert_ne!(keymap, merged_keyamp);

        let mut keymap = Keymaps::new(Box::new(Constant(merged_keyamp.clone())));
        assert_eq!(
            keymap.get(Mode::Normal, key!('i')),
            KeymapResult::Matched(MappableCommand::normal_mode),
            "Leaf should replace leaf"
        );
        assert_eq!(
            keymap.get(Mode::Normal, key!('无')),
            KeymapResult::Matched(MappableCommand::insert_mode),
            "New leaf should be present in merged keymap"
        );
        // Assumes that z is a node in the default keymap
        assert_eq!(
            keymap.get(Mode::Normal, key!('z')),
            KeymapResult::Matched(MappableCommand::jump_backward),
            "Leaf should replace node"
        );

        let keymap = merged_keyamp.get_mut(&Mode::Normal).unwrap();
        // Assumes that `g` is a node in default keymap
        assert_eq!(
            keymap.search(&[key!('g'), key!('$')]).unwrap(),
            &KeyTrie::MappableCommand(MappableCommand::goto_line_end),
            "Leaf should be present in merged subnode"
        );
        // Assumes that `gg` is in default keymap
        assert_eq!(
            keymap.search(&[key!('g'), key!('g')]).unwrap(),
            &KeyTrie::MappableCommand(MappableCommand::delete_char_forward),
            "Leaf should replace old leaf in merged subnode"
        );
        // Assumes that `ge` is in default keymap
        assert_eq!(
            keymap.search(&[key!('g'), key!('e')]).unwrap(),
            &KeyTrie::MappableCommand(MappableCommand::goto_last_line),
            "Old leaves in subnode should be present in merged node"
        );

        assert!(
            merged_keyamp
                .get(&Mode::Normal)
                .and_then(|key_trie| key_trie.node())
                .unwrap()
                .len()
                > 1
        );
        assert!(!merged_keyamp
            .get(&Mode::Insert)
            .and_then(|key_trie| key_trie.node())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn order_should_be_set() {
        let keymap = hashmap! {
            Mode::Normal => keymap!({ "Normal mode"
                "space" => { ""
                    "s" => { ""
                        "v" => vsplit,
                        "c" => hsplit,
                    },
                },
            })
        };
        let mut merged_keymap = default();
        merge_keys(&mut merged_keymap, keymap.clone());
        assert_ne!(keymap, merged_keymap);
        let keymap = merged_keymap.get_mut(&Mode::Normal).unwrap();
        // Make sure mapping works
        assert_eq!(
            keymap.search(&[key!(' '), key!('s'), key!('v')]).unwrap(),
            &KeyTrie::MappableCommand(MappableCommand::vsplit),
            "Leaf should be present in merged subnode"
        );
        // Merged nodes were ordered at the end
        let node = keymap.search(&[key!(' '), key!('s')]).unwrap();
        assert_eq!(
            node.node().unwrap().keys().copied().collect::<Vec<_>>(),
            vec![key!('v'), key!('c')]
        );
    }

    #[test]
    fn aliased_modes_are_same_in_default_keymap() {
        let keymaps = Keymaps::default().map();
        let root = keymaps.get(&Mode::Normal).unwrap();
        assert_eq!(
            root.search(&[key!(' '), key!('w')]).unwrap(),
            root.search(&["C-w".parse::<KeyEvent>().unwrap()]).unwrap(),
            "Mismatch for window mode on `Space-w` and `Ctrl-w`"
        );
        assert_eq!(
            root.search(&[key!('z')]).unwrap(),
            root.search(&[key!('Z')]).unwrap(),
            "Mismatch for view mode on `z` and `Z`"
        );
    }

    #[test]
    fn reverse_map() {
        let normal_mode = keymap!({ "Normal mode"
            "i" => insert_mode,
            "g" => { "Goto"
                "g" => goto_file_start,
                "e" => goto_file_end,
            },
            "j" | "k" => move_line_down,
        });
        let keymap = normal_mode;
        let mut reverse_map = keymap.reverse_map();

        // sort keybindings in order to have consistent tests
        // HashMaps can be compared but we can still get different ordering of bindings
        // for commands that have multiple bindings assigned
        for v in reverse_map.values_mut() {
            v.sort()
        }

        assert_eq!(
            reverse_map,
            HashMap::from([
                ("insert_mode".to_string(), vec![vec![key!('i')]]),
                (
                    "goto_file_start".to_string(),
                    vec![vec![key!('g'), key!('g')]]
                ),
                (
                    "goto_file_end".to_string(),
                    vec![vec![key!('g'), key!('e')]]
                ),
                (
                    "move_line_down".to_string(),
                    vec![vec![key!('j')], vec![key!('k')]]
                ),
            ]),
            "Mismatch"
        )
    }

    /// Deserialize into KeyTrieNode
    #[test]
    fn deserialize_node() {
        let keys = r#"
"+" = "select_all"
a = "append_mode"
        "#;
        let expectation = KeyTrie::Node(KeyTrieNode::new(
            "",
            indexmap! {
                key!('+') => KeyTrie::MappableCommand(
                    MappableCommand::select_all
                ),
                key!('a') => KeyTrie::MappableCommand(
                    MappableCommand::append_mode
                ),
            },
        ));

        assert_eq!(toml::from_str(keys), Ok(expectation));

        // Other fields in KeyTrieNode CANNOT be deserialized
        let invalid = r#"
name = "name"
is_sticky = false
        "#;
        let result = toml::from_str::<KeyTrieNode>(invalid);
        assert!(result.is_err_and(|error| error.message().contains("Invalid key code 'is_sticky'")));
    }

    #[test]
    fn escaped_keymap() {
        let keys = r#"
"+" = [
    "select_all",
    ":pipe sed -E 's/\\s+$//g'",
]
        "#;

        let key = KeyEvent {
            code: KeyCode::Char('+'),
            modifiers: KeyModifiers::NONE,
        };

        let expectation = KeyTrie::Node(KeyTrieNode::new(
            "",
            indexmap! {
                key => KeyTrie::Sequence(vec!{
                    MappableCommand::select_all,
                    MappableCommand::Typable {
                        name: "pipe".to_string(),
                        args: "sed -E 's/\\s+$//g'".to_string(),
                        doc: "".to_string(),
                    },
                })
            },
        ));

        assert_eq!(toml::from_str(keys), Ok(expectation));
    }
}
