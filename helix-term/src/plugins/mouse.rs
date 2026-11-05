//! Mouse events for plugins (API 0.5 `mouse`): each one is a call to the
//! listening plugin's "mouse" service, made right away, so a reply of
//! `true` can take the event from the editor.

use std::sync::atomic::{AtomicBool, Ordering};

use helix_event::runtime_local;
use helix_view::input::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use helix_view::Editor;
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::job::Jobs;

static ANY: AtomicBool = AtomicBool::new(false);

runtime_local! {
    /// The kinds each plugin hears.
    static LISTENERS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(Vec::new());
}

pub fn listen(plugin: String, kinds: Vec<String>) {
    let mut listeners = LISTENERS.lock();
    listeners.retain(|(owner, _)| *owner != plugin);
    if !kinds.is_empty() {
        listeners.push((plugin, kinds));
    }
    ANY.store(!listeners.is_empty(), Ordering::Relaxed);
}

/// Forget `plugin`'s listening (`None`: everyone's).
pub fn stop_plugin(plugin: Option<&str>) {
    let mut listeners = LISTENERS.lock();
    listeners.retain(|(owner, _)| plugin.is_some_and(|plugin| plugin != owner));
    ANY.store(!listeners.is_empty(), Ordering::Relaxed);
}

fn button(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
    }
}

/// The event as plugins see it: its kind and JSON. `None` for events in
/// no view and for moves.
fn describe(editor: &Editor, event: &MouseEvent) -> Option<(&'static str, Value)> {
    let (kind, button, direction) = match event.kind {
        MouseEventKind::Down(b) => ("down", Some(button(b)), None),
        MouseEventKind::Up(b) => ("up", Some(button(b)), None),
        MouseEventKind::Drag(b) => ("drag", Some(button(b)), None),
        MouseEventKind::ScrollUp => ("scroll", None, Some("up")),
        MouseEventKind::ScrollDown => ("scroll", None, Some("down")),
        MouseEventKind::ScrollLeft => ("scroll", None, Some("left")),
        MouseEventKind::ScrollRight => ("scroll", None, Some("right")),
        MouseEventKind::Moved => return None,
    };
    let (row, column) = (event.row, event.column);
    let (view, _) = editor.tree.views().find(|(view, _)| {
        let area = view.area;
        row >= area.top() && row < area.bottom() && column >= area.left() && column < area.right()
    })?;
    let doc = &editor.documents[&view.doc];
    let gutter = column < view.area.left() + view.gutter_offset(doc);
    let pos = if gutter {
        None
    } else {
        view.pos_at_screen_coords(doc, row, column, true)
    };
    // The line under the pointer, also in the gutter and past a line's end.
    let line = view
        .pos_at_screen_coords(doc, row, view.area.left() + view.gutter_offset(doc), true)
        .or(pos)
        .map(|pos| doc.text().char_to_line(pos));
    let modifiers: Vec<&str> = [
        (KeyModifiers::SHIFT, "shift"),
        (KeyModifiers::CONTROL, "ctrl"),
        (KeyModifiers::ALT, "alt"),
    ]
    .into_iter()
    .filter(|(modifier, _)| event.modifiers.contains(*modifier))
    .map(|(_, name)| name)
    .collect();
    Some((
        kind,
        json!({
            "button": button,
            "direction": direction,
            "view": super::view_number(view.id),
            "doc": super::plugin_doc_id(view.doc),
            "pos": pos,
            "line": line,
            "gutter": gutter,
            "row": row - view.area.top(),
            "column": column - view.area.left(),
            "modifiers": modifiers,
        }),
    ))
}

/// Tell the plugins listening about `event`; whether one took it.
pub fn handle(editor: &mut Editor, jobs: &mut Jobs, event: &MouseEvent) -> bool {
    if !ANY.load(Ordering::Relaxed) {
        return false;
    }
    let Some((kind, payload)) = describe(editor, event) else {
        return false;
    };
    let plugins: Vec<String> = LISTENERS
        .lock()
        .iter()
        .filter(|(_, kinds)| kinds.iter().any(|k| k == kind))
        .map(|(plugin, _)| plugin.clone())
        .collect();
    let payload = payload.to_string();
    for plugin in plugins {
        let call = super::editor_call(&plugin, "mouse", kind, payload.clone());
        match super::serve_now(editor, jobs, &call) {
            Ok(reply) if reply.trim() == "true" => return true,
            Ok(_) => {}
            Err(err) => log::warn!("{plugin}: mouse {kind}: {err}"),
        }
    }
    false
}
