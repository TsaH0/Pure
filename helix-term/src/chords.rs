//! Chords: a key pressed while another is held (`"p+s"` in
//! `[editor.chords.normal]`: hold `p`, press `s`). They need the terminal
//! to tell when keys are let go (the kitty keyboard protocol), which the
//! editor asks for when chords are set.
//!
//! A key that starts a chord waits: if a chord's second key comes while
//! it is held, the chord runs; if it is let go first, it does what it
//! would have done, then. Such a key doesn't repeat while held.

use std::str::FromStr;

use helix_event::runtime_local;
use helix_view::input::KeyEvent;
use helix_view::Editor;
use parking_lot::Mutex;
use termina::event::{KeyEvent as TermKey, KeyEventKind};

use crate::commands::{self, MappableCommand};
use crate::compositor::Compositor;
use crate::job::Jobs;

/// The key held that starts a chord: as it was pressed, and whether a
/// chord used it.
struct Held {
    key: TermKey,
    used: bool,
}

runtime_local! {
    static HELD: Mutex<Option<Held>> = Mutex::new(None);
}

/// `"p+s"` as its two keys.
fn parse(chord: &str) -> Option<(KeyEvent, KeyEvent)> {
    let (first, second) = chord.split_once('+')?;
    Some((
        KeyEvent::from_str(first.trim()).ok()?,
        KeyEvent::from_str(second.trim()).ok()?,
    ))
}

/// The chords in the current mode, as `(chord, commands)`: the key modes'
/// on in the focused buffer (the one turned on last first), then
/// `[editor.chords]`.
fn chords(editor: &Editor) -> Vec<(String, Vec<String>)> {
    let mode = editor.mode();
    let doc = helix_view::doc!(editor);
    let mut all = crate::keymap::modes::chords(Some(doc.id()), doc.language_name(), mode);
    if let Some(chords) = editor.config().chords.get(&mode) {
        all.extend(
            chords
                .iter()
                .map(|(chord, commands)| (chord.clone(), commands.commands().to_vec())),
        );
    }
    all
}

/// The commands of the chord `first+second` in the current mode.
fn chord(editor: &Editor, first: KeyEvent, second: KeyEvent) -> Option<Vec<String>> {
    chords(editor)
        .into_iter()
        .find_map(|(chord, commands)| (parse(&chord)? == (first, second)).then_some(commands))
}

/// Whether `key` starts a chord in the current mode.
fn starts_chord(editor: &Editor, key: KeyEvent) -> bool {
    chords(editor)
        .iter()
        .any(|(chord, _)| parse(chord).is_some_and(|(first, _)| first == key))
}

/// Whether there are chords to handle: in `[editor.chords]`, or in a key
/// mode that is on somewhere.
pub fn any(editor: &Editor) -> bool {
    !editor.config().chords.is_empty() || crate::keymap::modes::wants_key_releases()
}

runtime_local! {
    /// The terminal has told about a key being let go: it reports releases.
    /// Until then a key that starts a chord doesn't wait (it would wait for a
    /// release that never comes).
    static RELEASES_SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
}

fn same_key(a: &TermKey, b: &TermKey) -> bool {
    KeyEvent::from(*a).code == KeyEvent::from(*b).code
}

fn run(editor: &mut Editor, compositor: &mut Compositor, jobs: &mut Jobs, names: &[String]) {
    let mut commands = Vec::new();
    for name in names {
        match MappableCommand::from_str(name) {
            Ok(command) => commands.push(command),
            Err(err) => {
                editor.set_error(format!("chord: {err}"));
                return;
            }
        }
    }
    let mut cx = commands::Context {
        register: None,
        count: None,
        editor: &mut *editor,
        callback: Vec::new(),
        on_next_key_callback: None,
        jobs: &mut *jobs,
    };
    commands::execute_sequence(&mut cx, &commands);
    let callbacks = std::mem::take(&mut cx.callback);
    let mut cx = crate::compositor::Context {
        editor: &mut *editor,
        jobs: &mut *jobs,
        scroll: None,
    };
    for callback in callbacks {
        callback(compositor, &mut cx);
    }
}

/// A key event from the terminal: the key events to handle as usual in
/// its place (none when a chord took it or it waits).
pub fn filter(
    editor: &mut Editor,
    compositor: &mut Compositor,
    jobs: &mut Jobs,
    event: TermKey,
) -> Vec<TermKey> {
    // Chords are for the editor's own keys, not prompts and pickers.
    if event.kind == KeyEventKind::Release {
        RELEASES_SEEN.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let releases = RELEASES_SEEN.load(std::sync::atomic::Ordering::Relaxed);
    if !any(editor) || !releases || compositor.layer_count() > 1 {
        let held = HELD.lock().take();
        return held
            .filter(|held| !held.used)
            .map(|held| held.key)
            .into_iter()
            .chain((event.kind != KeyEventKind::Release).then_some(event))
            .collect();
    }
    let mut held = HELD.lock();
    match event.kind {
        KeyEventKind::Release => match held.take() {
            // Let go without a chord: what it does by itself.
            Some(first) if same_key(&first.key, &event) => {
                if first.used {
                    Vec::new()
                } else {
                    vec![first.key]
                }
            }
            other => {
                *held = other;
                Vec::new()
            }
        },
        // Held keys repeat: not the one that waits.
        KeyEventKind::Repeat
            if held
                .as_ref()
                .is_some_and(|first| same_key(&first.key, &event)) =>
        {
            Vec::new()
        }
        _ => {
            let key = KeyEvent::from(event);
            if let Some(first) = held.as_mut() {
                let first_key = KeyEvent::from(first.key);
                if let Some(commands) = chord(editor, first_key, key) {
                    first.used = true;
                    drop(held);
                    run(editor, compositor, jobs, &commands);
                    helix_event::request_redraw();
                    return Vec::new();
                }
                // Not a chord: the first key, then this one.
                let first = held.take().expect("held");
                drop(held);
                let mut keys = Vec::new();
                if !first.used {
                    keys.push(first.key);
                }
                keys.extend(filter(editor, compositor, jobs, event));
                return keys;
            }
            if starts_chord(editor, key) {
                *held = Some(Held {
                    key: event,
                    used: false,
                });
                return Vec::new();
            }
            vec![event]
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn chords_parse() {
        let key = |name: &str| KeyEvent::from_str(name).unwrap();
        assert_eq!(parse("p+s"), Some((key("p"), key("s"))));
        assert_eq!(parse("space + f"), Some((key("space"), key("f"))));
        assert_eq!(parse("C-x+j"), Some((key("C-x"), key("j"))));
        assert_eq!(parse("ps"), None);
    }
}
