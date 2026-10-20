//! Drawing a plugin's sidebar and handling its keys and mouse clicks.

use helix_core::unicode::width::UnicodeWidthStr;
use helix_view::graphics::Rect;
use helix_view::input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use helix_view::keyboard::{KeyCode, KeyModifiers};
use helix_view::Editor;
use tui::buffer::Buffer as Surface;

use crate::job::Jobs;

/// The sidebar's columns on the left of `area`, if one is shown (at most
/// half the width).
pub fn area(editor: &Editor, area: Rect) -> Option<Rect> {
    let sidebar = editor.sidebar.as_ref()?;
    let width = sidebar.width.min(area.width / 2);
    (width > 0).then(|| area.with_width(width))
}

pub fn render(editor: &mut Editor, area: Rect, surface: &mut Surface) {
    let theme = &editor.theme;
    let background = theme.get("ui.background");
    let border = theme.get("ui.window");
    let title_style = theme.get("ui.text.focus");
    let Some(sidebar) = editor.sidebar.as_mut() else {
        return;
    };
    surface.set_style(area, background);
    let inner = area.clip_right(1);
    // The right border.
    for y in area.top()..area.bottom() {
        surface[(area.right() - 1, y)]
            .set_symbol(tui::symbols::line::VERTICAL)
            .set_style(border);
    }
    surface.set_stringn(
        inner.x,
        inner.y,
        &sidebar.title,
        inner.width as usize,
        title_style,
    );
    let rows = inner.height.saturating_sub(1) as usize;
    sidebar.scroll_to_selection(rows);
    let selected_style = if sidebar.focused {
        theme.get("ui.menu.selected")
    } else {
        theme.get("ui.selection")
    };
    for (row, (index, item)) in sidebar
        .items
        .iter()
        .enumerate()
        .skip(sidebar.scroll)
        .take(rows)
        .enumerate()
    {
        let y = inner.y + 1 + row as u16;
        let indent = (item.depth as usize * 2).min(inner.width as usize);
        let style = if item.scope.is_empty() {
            theme.get("ui.text")
        } else {
            theme.get(&item.scope)
        };
        let width = inner.width as usize - indent;
        let text: String = if item.text.width() > width {
            let mut cut: String = item.text.chars().take(width.saturating_sub(1)).collect();
            cut.push('…');
            cut
        } else {
            item.text.clone()
        };
        surface.set_stringn(inner.x + indent as u16, y, &text, width, style);
        if index == sidebar.selected {
            surface.set_style(Rect::new(inner.x, y, inner.width, 1), selected_style);
        }
    }
}

/// Run the selected item's command.
fn activate(editor: &mut Editor, jobs: &mut Jobs) {
    let Some(sidebar) = editor.sidebar.as_ref() else {
        return;
    };
    let Some((command, args)) = sidebar
        .items
        .get(sidebar.selected)
        .and_then(|item| item.command.clone())
    else {
        return;
    };
    let name = format!("{}/{command}", sidebar.owner);
    if let Err(err) = crate::plugins::run_command(editor, jobs, &name, args) {
        editor.set_error(format!("{name}: {err}"));
    }
}

/// A key while the sidebar has focus. `false`: not the sidebar's key, the
/// editor handles it.
pub fn handle_key(editor: &mut Editor, jobs: &mut Jobs, key: KeyEvent, rows: usize) -> bool {
    let Some(sidebar) = editor.sidebar.as_mut() else {
        return false;
    };
    let half = (rows / 2).max(1) as isize;
    match (key.code, key.modifiers) {
        (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => sidebar.move_selection(1),
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => sidebar.move_selection(-1),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => sidebar.move_selection(half),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => sidebar.move_selection(-half),
        (KeyCode::Char('g') | KeyCode::Home, KeyModifiers::NONE) => sidebar.selected = 0,
        (KeyCode::Char('G'), _) | (KeyCode::End, _) => {
            sidebar.selected = sidebar.items.len().saturating_sub(1)
        }
        (KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right, KeyModifiers::NONE) => {
            activate(editor, jobs)
        }
        (KeyCode::Esc | KeyCode::Tab, KeyModifiers::NONE) => sidebar.focused = false,
        (KeyCode::Char('q'), KeyModifiers::NONE) => editor.sidebar = None,
        _ => return false,
    }
    helix_event::request_redraw();
    true
}

/// A mouse event in `area` (the sidebar's). Clicks select and run an item.
pub fn handle_mouse(editor: &mut Editor, jobs: &mut Jobs, event: &MouseEvent, area: Rect) -> bool {
    if event.column < area.x
        || event.column >= area.right()
        || event.row < area.y
        || event.row >= area.bottom()
    {
        return false;
    }
    let Some(sidebar) = editor.sidebar.as_mut() else {
        return false;
    };
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            sidebar.focused = true;
            let row = (event.row - area.y) as usize;
            if row == 0 {
                return true; // the title
            }
            let index = sidebar.scroll + row - 1;
            if index < sidebar.items.len() {
                sidebar.selected = index;
                activate(editor, jobs);
            }
        }
        MouseEventKind::ScrollDown => sidebar.move_selection(3),
        MouseEventKind::ScrollUp => sidebar.move_selection(-3),
        _ => {}
    }
    helix_event::request_redraw();
    true
}
