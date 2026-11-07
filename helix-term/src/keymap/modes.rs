//! Key modes, like Emacs' minor modes: named sets of key bindings that are
//! turned on and off while editing, in one buffer or in all of them.
//!
//! A mode comes from `[modes.<name>]` in `config.toml` (or a file in
//! `modes/` in the config directory, with the same tables) or from a plugin.
//! While on, its keys go over every other key: the user's `[keys]`, Pure's
//! and plugins'. Of several modes on at once, the one turned on last wins;
//! modes a language turns on (`languages`) and modes on by default rank
//! lowest, like Emacs' major modes. `conflicts` turns other modes off when
//! one goes on (Vim keys and Emacs keys, say) and `requires` turns modes on
//! with it.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use helix_event::runtime_local;
use helix_view::{document::Mode, DocumentId};
use parking_lot::Mutex;
use serde::Deserialize;

use super::{KeyTrie, KeyTrieNode};

/// A mode as configured.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct ModeConfig {
    pub description: String,
    /// Shown in the statusline (the `modes` element) while on; the name by
    /// default.
    pub lighter: Option<String>,
    /// On in every buffer at once (`:mode` turns it on and off everywhere),
    /// instead of in one buffer.
    pub global: bool,
    /// On from the start: everywhere for a global mode, in every buffer
    /// for another (until turned off there).
    pub enabled: bool,
    /// On in buffers of these languages.
    pub languages: Vec<String>,
    /// Turned off when this one goes on.
    pub conflicts: Vec<String>,
    /// Turned on with this one.
    pub requires: Vec<String>,
    /// Keys, as in `[keys]`: `[modes.vim.keys.normal]`.
    pub keys: HashMap<Mode, KeyTrie>,
    /// Commands run when it goes on and off, as in `[keys]` (`"insert_mode"`,
    /// `":set ..."`).
    pub on_enable: Vec<String>,
    pub on_disable: Vec<String>,
    /// Keys pressed while another is held, as `[editor.chords]`:
    /// `[modes.ki.chords.normal] "r+l" = [...]`. They need a terminal that
    /// reports key releases, which is asked for while such a mode is on.
    pub chords: HashMap<Mode, HashMap<String, helix_view::editor::ChordCommands>>,
}

/// Who defined a mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    Config,
    Plugin(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyMode {
    pub name: String,
    pub owner: Owner,
    pub config: ModeConfig,
}

impl KeyMode {
    pub fn lighter(&self) -> &str {
        self.config.lighter.as_deref().unwrap_or(&self.name)
    }

    /// On by itself in a buffer of `language`.
    fn default_on(&self, language: Option<&str>) -> bool {
        !self.config.global
            && (self.config.enabled
                || language
                    .is_some_and(|language| self.config.languages.iter().any(|l| l == language)))
    }
}

#[derive(Default)]
struct State {
    modes: Vec<Arc<KeyMode>>,
    /// Turned on, with when (later ones win).
    global: Vec<(String, u64)>,
    local: HashMap<DocumentId, Vec<(String, u64)>>,
    /// Turned off in a buffer although on by default or globally.
    off: HashMap<DocumentId, Vec<String>>,
    clock: u64,
    /// Changes with every change, for the composed keymap's cache.
    generation: u64,
}

type CacheKey = (usize, u64, Vec<String>);

runtime_local! {
    static STATE: Mutex<Option<State>> = Mutex::new(None);
    /// Whether any mode could be on anywhere: the keymap is composed only
    /// then.
    static ANY: AtomicBool = AtomicBool::new(false);
    /// Whether some mode is about languages (key lookups then need the
    /// document's).
    static LANGUAGES: AtomicBool = AtomicBool::new(false);
    static COMPOSED: Mutex<Option<(CacheKey, Arc<HashMap<Mode, KeyTrie>>)>> = Mutex::new(None);
}

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut state = STATE.lock();
    let state = state.get_or_insert_with(State::default);
    let result = f(state);
    state.generation += 1;
    ANY.store(
        !state.global.is_empty()
            || state.local.values().any(|modes| !modes.is_empty())
            || state.modes.iter().any(|mode| {
                !mode.config.global && (mode.config.enabled || !mode.config.languages.is_empty())
            }),
        Ordering::Relaxed,
    );
    LANGUAGES.store(
        state
            .modes
            .iter()
            .any(|mode| !mode.config.languages.is_empty()),
        Ordering::Relaxed,
    );
    result
}

fn read<T>(f: impl FnOnce(&State) -> T) -> T {
    let state = STATE.lock();
    match &*state {
        Some(state) => f(state),
        None => f(&State::default()),
    }
}

/// Whether key lookups need the document's language for modes.
pub fn need_language() -> bool {
    LANGUAGES.load(Ordering::Relaxed)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
}

impl State {
    fn find(&self, name: &str) -> Option<&Arc<KeyMode>> {
        self.modes.iter().find(|mode| mode.name == name)
    }

    fn insert(&mut self, mode: KeyMode) {
        let enable = mode.config.global && mode.config.enabled;
        let name = mode.name.clone();
        self.modes.retain(|other| other.name != mode.name);
        self.modes.push(Arc::new(mode));
        if enable && !self.global.iter().any(|(on, _)| *on == name) {
            self.global.push((name, 0));
        }
    }

    fn active(&self, doc: Option<DocumentId>, language: Option<&str>) -> Vec<Arc<KeyMode>> {
        let off = doc.and_then(|doc| self.off.get(&doc));
        let is_off = |name: &str| off.is_some_and(|off| off.iter().any(|o| o == name));
        let mut on: BTreeMap<&str, u64> = BTreeMap::new();
        for mode in &self.modes {
            if mode.default_on(language) && !is_off(&mode.name) {
                on.insert(&mode.name, 0);
            }
        }
        let local = doc.and_then(|doc| self.local.get(&doc));
        for (name, when) in self.global.iter().chain(local.into_iter().flatten()) {
            if !is_off(name) {
                let entry = on.entry(name).or_insert(*when);
                *entry = (*entry).max(*when);
            }
        }
        let mut active: Vec<_> = on
            .into_iter()
            .filter_map(|(name, when)| Some((when, self.find(name)?.clone())))
            .collect();
        // Lowest precedence first; ties in definition order.
        active.sort_by_key(|(when, mode)| {
            (*when, self.modes.iter().position(|m| m.name == mode.name))
        });
        active.into_iter().map(|(_, mode)| mode).collect()
    }

    fn is_on(&self, doc: Option<DocumentId>, language: Option<&str>, name: &str) -> bool {
        self.active(doc, language)
            .iter()
            .any(|mode| mode.name == name)
    }

    /// Turn `name` on; the commands to run, in order.
    fn enable(
        &mut self,
        doc: Option<DocumentId>,
        language: Option<&str>,
        name: &str,
        seen: &mut Vec<String>,
    ) -> Result<Vec<String>, String> {
        if seen.iter().any(|s| s == name) {
            return Err(format!("modes require each other: {}", seen.join(" -> ")));
        }
        seen.push(name.to_string());
        let mode = self
            .find(name)
            .cloned()
            .ok_or_else(|| format!("no mode '{name}'"))?;
        let mut commands = Vec::new();
        for required in &mode.config.requires {
            if !self.is_on(doc, language, required) {
                commands.extend(self.enable(doc, language, required, seen)?);
            }
        }
        for conflict in &mode.config.conflicts {
            if self.is_on(doc, language, conflict) {
                commands.extend(self.disable(doc, language, conflict)?);
            }
        }
        self.clock += 1;
        let when = self.clock;
        if mode.config.global {
            self.global.retain(|(on, _)| on != name);
            self.global.push((name.to_string(), when));
            // On everywhere again, also where it was turned off.
            for off in self.off.values_mut() {
                off.retain(|o| o != name);
            }
        } else {
            let doc = doc.ok_or("a buffer mode needs a buffer")?;
            let local = self.local.entry(doc).or_default();
            local.retain(|(on, _)| on != name);
            local.push((name.to_string(), when));
            if let Some(off) = self.off.get_mut(&doc) {
                off.retain(|o| o != name);
            }
        }
        commands.extend(mode.config.on_enable.iter().cloned());
        Ok(commands)
    }

    fn disable(
        &mut self,
        doc: Option<DocumentId>,
        language: Option<&str>,
        name: &str,
    ) -> Result<Vec<String>, String> {
        let mode = self
            .find(name)
            .cloned()
            .ok_or_else(|| format!("no mode '{name}'"))?;
        if !self.is_on(doc, language, name) {
            return Ok(Vec::new());
        }
        // Modes that need this one go off first.
        let mut commands = Vec::new();
        let dependents: Vec<String> = self
            .active(doc, language)
            .iter()
            .filter(|other| other.config.requires.iter().any(|r| r == name))
            .map(|other| other.name.clone())
            .collect();
        for dependent in dependents {
            commands.extend(self.disable(doc, language, &dependent)?);
        }
        if mode.config.global {
            self.global.retain(|(on, _)| on != name);
        } else if let Some(doc) = doc {
            if let Some(local) = self.local.get_mut(&doc) {
                local.retain(|(on, _)| on != name);
            }
            if mode.default_on(language) {
                let off = self.off.entry(doc).or_default();
                if !off.iter().any(|o| o == name) {
                    off.push(name.to_string());
                }
            }
        }
        commands.extend(mode.config.on_disable.iter().cloned());
        Ok(commands)
    }
}

/// Define (or redefine) a mode.
pub fn define(mode: KeyMode) -> Result<(), String> {
    if !valid_name(&mode.name) {
        return Err(format!(
            "invalid mode name '{}': use letters, digits, '-', '_', ':' and '.'",
            mode.name
        ));
    }
    with_state(|state| state.insert(mode));
    Ok(())
}

/// The modes from the config (`[modes]` and `modes/*.toml`), replacing the
/// config's earlier ones. What is on stays on.
pub fn set_config_modes(modes: &BTreeMap<String, ModeConfig>) -> Vec<String> {
    let mut errors = Vec::new();
    with_state(|state| {
        state.modes.retain(|mode| mode.owner != Owner::Config);
        for (name, config) in modes {
            if !valid_name(name) {
                errors.push(format!("invalid mode name '{name}'"));
                continue;
            }
            state.insert(KeyMode {
                name: name.clone(),
                owner: Owner::Config,
                config: config.clone(),
            });
        }
        let names: Vec<String> = state.modes.iter().map(|m| m.name.clone()).collect();
        for mode in &state.modes {
            for other in mode.config.conflicts.iter().chain(&mode.config.requires) {
                if !names.contains(other) {
                    errors.push(format!("mode '{}': no mode '{other}'", mode.name));
                }
            }
        }
        forget_missing(state);
    });
    *COMPOSED.lock() = None;
    errors
}

/// Remove `plugin`'s modes (`None`: every plugin's).
pub fn remove_plugin(plugin: Option<&str>) {
    with_state(|state| {
        state.modes.retain(|mode| match (&mode.owner, plugin) {
            (Owner::Plugin(_), None) => false,
            (Owner::Plugin(owner), Some(plugin)) => owner != plugin,
            (Owner::Config, _) => true,
        });
        forget_missing(state);
    });
}

fn forget_missing(state: &mut State) {
    let exists = |name: &str| state.modes.iter().any(|m| m.name == name);
    let global = std::mem::take(&mut state.global);
    state.global = global.into_iter().filter(|(n, _)| exists(n)).collect();
    let mut local = std::mem::take(&mut state.local);
    for modes in local.values_mut() {
        modes.retain(|(n, _)| exists(n));
    }
    state.local = local;
}

/// Forget the modes of buffers when they close.
pub fn register_hooks() {
    use helix_view::events::DocumentDidClose;
    helix_event::register_hook!(move |event: &mut DocumentDidClose<'_>| {
        forget_document(event.doc.id());
        Ok(())
    });
}

/// A buffer closed: forget its modes.
pub fn forget_document(doc: DocumentId) {
    if STATE.lock().is_none() {
        return;
    }
    with_state(|state| {
        state.local.remove(&doc);
        state.off.remove(&doc);
    });
}

/// What `:mode` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    On,
    Off,
    Toggle,
}

/// Turn mode `name` on or off in `doc` (everywhere for a global mode):
/// whether it is now on, and the commands to run (`on-enable`,
/// `on-disable`, of it and the modes it turned on or off).
pub fn switch(
    doc: Option<DocumentId>,
    language: Option<&str>,
    name: &str,
    how: Switch,
) -> Result<(bool, Vec<String>), String> {
    with_state(|state| {
        let on = state.is_on(doc, language, name);
        let turn_on = match how {
            Switch::On => true,
            Switch::Off => false,
            Switch::Toggle => !on,
        };
        let commands = if turn_on {
            state.enable(doc, language, name, &mut Vec::new())?
        } else {
            state.disable(doc, language, name)?
        };
        Ok((turn_on, commands))
    })
}

/// The modes on in `doc`, lowest precedence first.
pub fn active(doc: Option<DocumentId>, language: Option<&str>) -> Vec<Arc<KeyMode>> {
    if !ANY.load(Ordering::Relaxed) {
        return Vec::new();
    }
    read(|state| state.active(doc, language))
}

/// Every mode, in definition order.
pub fn all() -> Vec<Arc<KeyMode>> {
    read(|state| state.modes.clone())
}

pub fn find(name: &str) -> Option<Arc<KeyMode>> {
    read(|state| state.find(name).cloned())
}

/// `keymap` with the keys of the modes on in `doc` over it; `None` when
/// none is (the usual case: one atomic load).
pub fn compose(
    keymap: &HashMap<Mode, KeyTrie>,
    doc: Option<DocumentId>,
    language: Option<&str>,
) -> Option<Arc<HashMap<Mode, KeyTrie>>> {
    if !ANY.load(Ordering::Relaxed) {
        return None;
    }
    let (generation, active) = read(|state| (state.generation, state.active(doc, language)));
    if active.is_empty() {
        return None;
    }
    let names: Vec<String> = active.iter().map(|mode| mode.name.clone()).collect();
    let key = (keymap as *const _ as usize, generation, names);
    if let Some((cached, map)) = &*COMPOSED.lock() {
        if *cached == key {
            return Some(map.clone());
        }
    }
    let mut map = keymap.clone();
    for mode in &active {
        for (vim_mode, keys) in &mode.config.keys {
            let KeyTrie::Node(keys) = keys else { continue };
            match map.get_mut(vim_mode) {
                Some(KeyTrie::Node(base)) => base.merge(keys.clone()),
                _ => {
                    map.insert(*vim_mode, KeyTrie::Node(keys.clone()));
                }
            }
        }
    }
    let map = Arc::new(map);
    *COMPOSED.lock() = Some((key, map.clone()));
    Some(map)
}

/// The chords of the modes on in `doc` in `vim_mode`, highest precedence
/// first, as `(chord, commands)`.
pub fn chords(
    doc: Option<DocumentId>,
    language: Option<&str>,
    vim_mode: Mode,
) -> Vec<(String, Vec<String>)> {
    let mut active = active(doc, language);
    active.reverse();
    active
        .iter()
        .filter_map(|mode| mode.config.chords.get(&vim_mode))
        .flat_map(|chords| {
            chords
                .iter()
                .map(|(chord, commands)| (chord.clone(), commands.commands().to_vec()))
        })
        .collect()
}

/// Whether a mode with chords is on anywhere (the terminal is then asked
/// for key releases).
pub fn wants_key_releases() -> bool {
    if !ANY.load(Ordering::Relaxed) {
        return false;
    }
    read(|state| {
        let has = |name: &str| {
            state
                .find(name)
                .is_some_and(|mode| !mode.config.chords.is_empty())
        };
        state.global.iter().any(|(name, _)| has(name))
            || state.local.values().flatten().any(|(name, _)| has(name))
            || state.modes.iter().any(|mode| {
                // On by itself somewhere (it may be).
                !mode.config.chords.is_empty()
                    && !mode.config.global
                    && (mode.config.enabled || !mode.config.languages.is_empty())
            })
    })
}

/// The keymap changed (config reload).
pub fn forget_composed() {
    *COMPOSED.lock() = None;
}

/// The statusline's `modes` element: the lighters of the modes on, or
/// nothing.
pub fn lighters(doc: Option<DocumentId>, language: Option<&str>) -> String {
    let active = active(doc, language);
    active
        .iter()
        .map(|mode| mode.lighter())
        .filter(|lighter| !lighter.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every key a mode binds, as `(mode, keys, what)`, for `:describe-mode`.
pub fn describe(mode: &KeyMode) -> Vec<(Mode, String, String)> {
    fn walk(
        node: &KeyTrieNode,
        prefix: &mut Vec<String>,
        vim_mode: Mode,
        out: &mut Vec<(Mode, String, String)>,
    ) {
        let mut keys: Vec<_> = node.iter().collect();
        keys.sort_by_key(|(key, _)| key.to_string());
        for (key, trie) in keys {
            prefix.push(key.to_string());
            match trie {
                KeyTrie::Node(node) => walk(node, prefix, vim_mode, out),
                KeyTrie::MappableCommand(command) => {
                    out.push((vim_mode, prefix.join(" "), command.to_string()))
                }
                KeyTrie::Sequence(commands) => out.push((
                    vim_mode,
                    prefix.join(" "),
                    commands
                        .iter()
                        .map(|c| c.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                )),
            }
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    let mut vim_modes: Vec<_> = mode.config.keys.iter().collect();
    vim_modes.sort_by_key(|(vim_mode, _)| vim_mode.to_string());
    for (vim_mode, keys) in vim_modes {
        if let KeyTrie::Node(node) = keys {
            walk(node, &mut Vec::new(), *vim_mode, &mut out);
        }
    }
    out
}

/// Keys for a mode from `(mode, keys, command)` triples, keys and
/// commands written as in `[keys]` ("C-x C-s", ":write").
pub fn keys_from_bindings(
    bindings: &[(Mode, String, String)],
) -> Result<HashMap<Mode, KeyTrie>, String> {
    use std::str::FromStr;
    let mut map: HashMap<Mode, KeyTrie> = HashMap::new();
    for (vim_mode, keys, command) in bindings {
        let keys: Vec<helix_view::input::KeyEvent> = keys
            .split_whitespace()
            .map(|key| {
                helix_view::input::KeyEvent::from_str(key)
                    .map_err(|err| format!("key '{key}': {err}"))
            })
            .collect::<Result<_, _>>()?;
        let Some((last, prefix)) = keys.split_last() else {
            return Err("a binding without keys".into());
        };
        let command = super::MappableCommand::from_str(command)
            .map_err(|err| format!("command '{command}': {err}"))?;
        let mut node = match map
            .entry(*vim_mode)
            .or_insert_with(|| KeyTrie::Node(KeyTrieNode::default()))
        {
            KeyTrie::Node(node) => node,
            _ => unreachable!(),
        };
        for key in prefix {
            let entry = node
                .map
                .entry(*key)
                .or_insert_with(|| KeyTrie::Node(KeyTrieNode::default()));
            if !matches!(entry, KeyTrie::Node(_)) {
                *entry = KeyTrie::Node(KeyTrieNode::default());
            }
            node = match entry {
                KeyTrie::Node(node) => node,
                _ => unreachable!(),
            };
        }
        node.map.insert(*last, KeyTrie::MappableCommand(command));
    }
    Ok(map)
}

/// Read the `modes/*.toml` files in `dir`: `[modes.<name>]` tables, as in
/// `config.toml`. Errors name the file.
pub fn read_mode_files(dir: &std::path::Path) -> (BTreeMap<String, ModeConfig>, Vec<String>) {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct File {
        #[serde(default)]
        modes: BTreeMap<String, ModeConfig>,
    }
    let mut modes = BTreeMap::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (modes, errors);
    };
    let mut paths: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();
    for path in paths {
        let parsed = std::fs::read_to_string(&path)
            .map_err(|err| err.to_string())
            .and_then(|text| toml::from_str::<File>(&text).map_err(|err| err.to_string()));
        match parsed {
            Ok(file) => modes.extend(file.modes),
            Err(err) => errors.push(format!("{}: {err}", path.display())),
        }
    }
    (modes, errors)
}

#[cfg(test)]
mod test {
    use super::*;
    use helix_view::input::KeyEvent;
    use std::str::FromStr;

    fn mode(toml: &str) -> ModeConfig {
        toml::from_str(toml).unwrap()
    }

    fn command_at(map: &HashMap<Mode, KeyTrie>, keys: &str) -> Option<String> {
        let keys: Vec<KeyEvent> = keys
            .split(' ')
            .map(|k| KeyEvent::from_str(k).unwrap())
            .collect();
        match map[&Mode::Normal].search(&keys)? {
            KeyTrie::MappableCommand(command) => Some(command.name().to_string()),
            _ => None,
        }
    }

    fn modes_go_over_keys_and_the_last_on_wins() {
        let doc = DocumentId::default();
        let mut modes = BTreeMap::new();
        modes.insert(
            "one".to_string(),
            mode("[keys.normal]\nx = \"goto_line_end\"\ng.q = \"goto_file_start\""),
        );
        modes.insert(
            "two".to_string(),
            mode("conflicts = []\n[keys.normal]\nx = \"goto_line_start\""),
        );
        modes.insert(
            "rust-only".to_string(),
            mode("languages = [\"rust\"]\n[keys.normal]\ny = \"goto_line_start\""),
        );
        assert!(set_config_modes(&modes).is_empty());
        let base = super::super::default();
        assert!(compose(&base, Some(doc), None).is_none());
        assert_eq!(command_at(&base, "x"), Some("extend_line_below".into()));

        switch(Some(doc), None, "one", Switch::On).unwrap();
        let map = compose(&base, Some(doc), None).unwrap();
        assert_eq!(command_at(&map, "x"), Some("goto_line_end".into()));
        // Menus merge: `g q` added, `g g` kept.
        assert_eq!(command_at(&map, "g q"), Some("goto_file_start".into()));
        assert_eq!(command_at(&map, "g g"), Some("goto_file_start".into()));
        switch(Some(doc), None, "two", Switch::On).unwrap();
        let map = compose(&base, Some(doc), None).unwrap();
        assert_eq!(command_at(&map, "x"), Some("goto_line_start".into()));
        assert_eq!(lighters(Some(doc), None), "one two");

        // Language modes, and turning them off in one buffer.
        assert_eq!(lighters(Some(doc), Some("rust")), "rust-only one two");
        switch(Some(doc), Some("rust"), "rust-only", Switch::Off).unwrap();
        assert_eq!(lighters(Some(doc), Some("rust")), "one two");
        forget_document(doc);
        assert_eq!(lighters(Some(doc), None), "");
    }

    fn conflicts_requires_and_commands() {
        let doc = DocumentId::default();
        let mut modes = BTreeMap::new();
        modes.insert(
            "vim".to_string(),
            mode("global = true\nconflicts = [\"emacs\"]\nrequires = [\"base\"]\non-enable = [\"normal_mode\"]"),
        );
        modes.insert(
            "emacs".to_string(),
            mode("global = true\nconflicts = [\"vim\"]\non-enable = [\"insert_mode\"]\non-disable = [\"normal_mode\"]"),
        );
        modes.insert("base".to_string(), mode("global = true\nlighter = \"\""));
        modes.insert("loop-a".to_string(), mode("requires = [\"loop-b\"]"));
        modes.insert("loop-b".to_string(), mode("requires = [\"loop-a\"]"));
        assert!(set_config_modes(&modes).is_empty());
        let (on, commands) = switch(Some(doc), None, "emacs", Switch::On).unwrap();
        assert!(on);
        assert_eq!(commands, ["insert_mode"]);
        let (_, commands) = switch(Some(doc), None, "vim", Switch::Toggle).unwrap();
        // emacs off (its on-disable), base on, then vim's on-enable.
        assert_eq!(commands, ["normal_mode", "normal_mode"]);
        // Global: on in every buffer; an empty lighter shows nothing.
        assert_eq!(lighters(None, None), "vim");
        // Off: base goes, and vim with it (it needs base).
        switch(Some(doc), None, "base", Switch::Off).unwrap();
        assert_eq!(lighters(None, None), "");
        assert!(switch(Some(doc), None, "loop-a", Switch::On)
            .unwrap_err()
            .contains("require each other"));
        assert!(switch(Some(doc), None, "nope", Switch::On).is_err());
        let mut bad = BTreeMap::new();
        bad.insert("x".to_string(), mode("conflicts = [\"ghost\"]"));
        assert_eq!(set_config_modes(&bad), ["mode 'x': no mode 'ghost'"]);
    }

    /// One test: the modes' state is shared by the tests' threads.
    #[test]
    fn key_modes() {
        modes_go_over_keys_and_the_last_on_wins();
        conflicts_requires_and_commands();
        shipped_modes_parse_and_refer_to_real_modes();
    }

    fn shipped_modes_parse_and_refer_to_real_modes() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/modes");
        let (modes, errors) = read_mode_files(&dir);
        assert!(errors.is_empty(), "{errors:?}");
        for name in ["vim", "emacs", "ki", "ki-word", "ki-node"] {
            assert!(modes.contains_key(name), "{name}");
        }
        assert!(set_config_modes(&modes).is_empty());
    }
}
