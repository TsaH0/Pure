//! Example plugin (API 0.5): `f`, `F`, `t` and `T` reimplemented by a
//! plugin, like Vim's: they search the cursor's line only (`same-line`),
//! optionally smart-case, and `repeat` / `repeat-reverse` (Vim's `;` and
//! `,`) redo the last one. Nothing in the editor changes: the plugin binds
//! the keys above Pure's defaults and asks for the key typed after them
//! (`keymaps::then_next_key`), as `f` does natively.

use std::cell::{Cell, RefCell};

use helix_plugin_api::{
    editor,
    keymaps::{self, binding, then_next_key},
    register_plugin, CommandInfo, Event, KeyOutcome, Mode, Plugin, Range, Registration,
};

#[derive(Debug, Clone, Copy, PartialEq)]
struct Find {
    forward: bool,
    /// `t`/`T`: stop next to the character.
    till: bool,
    target: char,
}

thread_local! {
    static SAME_LINE: Cell<bool> = const { Cell::new(true) };
    static SMART_CASE: Cell<bool> = const { Cell::new(false) };
    static LAST: RefCell<Option<Find>> = const { RefCell::new(None) };
}

/// The character a key name stands for.
fn key_char(key: &str) -> Option<char> {
    match key {
        "space" => Some(' '),
        "tab" => Some('\t'),
        "ret" => Some('\n'),
        "minus" => Some('-'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        _ => {
            let mut chars = key.chars();
            let c = chars.next()?;
            chars.next().is_none().then_some(c)
        }
    }
}

fn matches(found: char, target: char) -> bool {
    found == target
        || SMART_CASE.get()
            && target.is_lowercase()
            && found.to_lowercase().eq(target.to_lowercase())
}

/// Where `find` lands from `cursor`, the `count`th match.
fn land(text: &[char], cursor: usize, find: Find, count: usize, repeat: bool) -> Option<usize> {
    let same_line = SAME_LINE.get();
    let mut pos = cursor;
    // Repeating `t` from next to its character would find that one again.
    let skip = usize::from(repeat && find.till);
    for n in 0..count {
        let first = n == 0;
        pos = if find.forward {
            let start = pos + 1 + if first { skip } else { 0 };
            (start..text.len())
                .take_while(|i| !same_line || text[*i] != '\n' || find.target == '\n')
                .find(|i| matches(text[*i], find.target))?
        } else {
            let end = pos.checked_sub(if first { skip } else { 0 })?;
            (0..end)
                .rev()
                .take_while(|i| !same_line || text[*i] != '\n')
                .find(|i| matches(text[*i], find.target))?
        };
    }
    Some(match (find.forward, find.till) {
        (true, true) => pos - 1,
        (false, true) => pos + 1,
        _ => pos,
    })
}

/// Move (or extend, in select mode) every selection.
fn run(find: Find, count: usize, repeat: bool) -> Result<(), String> {
    let doc = editor::current_document().ok_or("no document")?;
    let text: Vec<char> = editor::text(doc)?.chars().collect();
    let extend = editor::current_mode() == Mode::Select;
    let ranges = editor::selections(doc)?
        .into_iter()
        .map(|range| {
            // The cursor is on the character before the head when the
            // range goes forward.
            let cursor = if range.head > range.anchor {
                range.head - 1
            } else {
                range.head
            } as usize;
            let Some(to) = land(&text, cursor, find, count, repeat) else {
                return range;
            };
            let to = to as u32;
            let cursor = cursor as u32;
            match (extend, to >= cursor) {
                (true, _) => Range {
                    anchor: range.anchor,
                    head: if to >= range.anchor { to + 1 } else { to },
                },
                // Like Helix: from the cursor to the character, inclusive.
                (false, true) => Range {
                    anchor: cursor,
                    head: to + 1,
                },
                (false, false) => Range {
                    anchor: cursor + 1,
                    head: to,
                },
            }
        })
        .collect::<Vec<_>>();
    editor::set_selections(doc, &ranges)
}

fn start(forward: bool, till: bool, count: Option<u32>) -> Result<KeyOutcome, String> {
    let count = count.unwrap_or(1).max(1) as usize;
    then_next_key(move |key| {
        let Some(target) = key_char(key) else {
            // Escape and the like: nothing.
            return Ok(());
        };
        let find = Find {
            forward,
            till,
            target,
        };
        LAST.set(Some(find));
        run(find, count, false)
    })?;
    Ok(KeyOutcome::Handled)
}

fn configure(config: &str) {
    let config: serde_json::Value = serde_json::from_str(config).unwrap_or_default();
    if let Some(same_line) = config["same-line"].as_bool() {
        SAME_LINE.set(same_line);
    }
    if let Some(smart_case) = config["smart-case"].as_bool() {
        SMART_CASE.set(smart_case);
    }
}

struct FindChar;

impl Plugin for FindChar {
    fn init(config: String) -> Result<Registration, String> {
        configure(&config);
        for mode in [Mode::Normal, Mode::Select] {
            for (key, forward, till, doc) in [
                ("f", true, false, "Find a character forward"),
                ("F", false, false, "Find a character backward"),
                ("t", true, true, "Till a character forward"),
                ("T", false, true, "Till a character backward"),
            ] {
                keymaps::bind(binding(mode, key, key, doc), move |_keys, count| {
                    start(forward, till, count)
                })?;
            }
        }
        let command = |name: &str, doc: &str| CommandInfo {
            name: name.into(),
            doc: doc.into(),
        };
        Ok(Registration {
            commands: vec![
                command("repeat", "Repeat the last f, F, t or T (Vim's ;)"),
                command("repeat-reverse", "Repeat it the other way (Vim's ,)"),
            ],
            events: vec![],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        let last = LAST.with_borrow(|last| *last).ok_or("nothing to repeat")?;
        match name {
            "repeat" => run(last, 1, true),
            "repeat-reverse" => run(
                Find {
                    forward: !last.forward,
                    ..last
                },
                1,
                true,
            ),
            _ => Err(format!("unknown command {name}")),
        }
    }

    fn on_event(event: Event) {
        if let Event::ConfigChange(config) = event {
            configure(&config);
        }
    }
}

register_plugin!(FindChar);
