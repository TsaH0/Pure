//! Drawing plugin panels (the sidebar is the left one) and handling their
//! keys and mouse clicks.

use helix_core::unicode::width::UnicodeWidthStr;
use helix_view::graphics::Rect;
use helix_view::input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use helix_view::keyboard::{KeyCode, KeyModifiers};
use helix_view::sidebar::Dock;
use helix_view::Editor;
use tui::buffer::Buffer as Surface;

use crate::job::Jobs;

/// Split `area` between the panels and the views: side panels take columns
/// (at most a third of the width each, half if alone), the bottom one rows
/// below the views (at most half the height). Returns the views' area.
pub fn layout(editor: &Editor, area: Rect) -> (Rect, Vec<(Dock, Rect)>) {
    let mut views = area;
    let mut areas = Vec::new();
    let both = editor.panel(Dock::Left).is_some() && editor.panel(Dock::Right).is_some();
    let side_max = if both { area.width / 3 } else { area.width / 2 };
    if let Some(panel) = editor.panel(Dock::Left) {
        let width = panel.width.min(side_max);
        if width > 0 {
            areas.push((Dock::Left, views.with_width(width)));
            views = views.clip_left(width);
        }
    }
    if let Some(panel) = editor.panel(Dock::Right) {
        let width = panel.width.min(side_max);
        if width > 0 {
            areas.push((Dock::Right, views.clip_left(views.width - width)));
            views = views.clip_right(width);
        }
    }
    if let Some(panel) = editor.panel(Dock::Bottom) {
        let height = panel.width.min(area.height / 2);
        if height > 1 {
            areas.push((Dock::Bottom, views.clip_top(views.height - height)));
            views = views.clip_bottom(height);
        }
    }
    (views, areas)
}

/// Rows of items `area` shows for a panel at `dock` (without the title).
pub fn rows(dock: Dock, area: Rect) -> usize {
    let _ = dock;
    area.height.saturating_sub(1) as usize
}

pub fn render(editor: &mut Editor, dock: Dock, area: Rect, surface: &mut Surface) {
    let theme = &editor.theme;
    let background = theme.get("ui.background");
    let border = theme.get("ui.window");
    let title_style = theme.get("ui.text.focus");
    let text_style = theme.get("ui.text");
    let focused_style = theme.get("ui.menu.selected");
    let unfocused_style = theme.get("ui.selection");
    // Fields, not methods: the theme stays borrowed.
    let Some(panel) = editor.panels.iter_mut().find(|panel| panel.dock == dock) else {
        return;
    };
    surface.set_style(area, background);
    // The border on the side of the views; the bottom panel's top border
    // carries the title.
    let (inner, title_at) = match dock {
        Dock::Left => {
            for y in area.top()..area.bottom() {
                surface[(area.right() - 1, y)]
                    .set_symbol(tui::symbols::line::VERTICAL)
                    .set_style(border);
            }
            (area.clip_right(1), None)
        }
        Dock::Right => {
            for y in area.top()..area.bottom() {
                surface[(area.left(), y)]
                    .set_symbol(tui::symbols::line::VERTICAL)
                    .set_style(border);
            }
            (area.clip_left(1), None)
        }
        Dock::Bottom => {
            for x in area.left()..area.right() {
                surface[(x, area.top())]
                    .set_symbol(tui::symbols::line::HORIZONTAL)
                    .set_style(border);
            }
            (area.clip_top(1), Some(area.y))
        }
    };
    let (title_y, list_top) = match title_at {
        Some(y) => (y, inner.y),
        None => (inner.y, inner.y + 1),
    };
    let title = if title_at.is_some() {
        format!(" {} ", panel.title)
    } else {
        panel.title.clone()
    };
    let title_x = if title_at.is_some() {
        inner.x + 1
    } else {
        inner.x
    };
    surface.set_stringn(title_x, title_y, &title, inner.width as usize, title_style);
    let rows = inner.bottom().saturating_sub(list_top) as usize;
    panel.scroll_to_selection(rows);
    let selected_style = if panel.focused {
        focused_style
    } else {
        unfocused_style
    };
    for (row, (index, item)) in panel
        .items
        .iter()
        .enumerate()
        .skip(panel.scroll)
        .take(rows)
        .enumerate()
    {
        let y = list_top + row as u16;
        let indent = (item.depth as usize * 2).min(inner.width as usize);
        let style = if item.scope.is_empty() {
            text_style
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
        if index == panel.selected {
            surface.set_style(Rect::new(inner.x, y, inner.width, 1), selected_style);
        }
    }
}

/// Run the selected item's command of the panel at `dock`.
fn activate(editor: &mut Editor, jobs: &mut Jobs, dock: Dock) {
    let Some(panel) = editor.panel(dock) else {
        return;
    };
    let Some((command, args)) = panel
        .items
        .get(panel.selected)
        .and_then(|item| item.command.clone())
    else {
        return;
    };
    let name = format!("{}/{command}", panel.owner);
    if let Err(err) = crate::plugins::run_command(editor, jobs, &name, args) {
        editor.set_error(format!("{name}: {err}"));
    }
}

/// A key while a panel has focus (`rows` of items shown). `false`: not
/// the panel's key, the editor handles it.
pub fn handle_key(editor: &mut Editor, jobs: &mut Jobs, key: KeyEvent, rows: usize) -> bool {
    let Some(panel) = editor.focused_panel_mut() else {
        return false;
    };
    let dock = panel.dock;
    let half = (rows / 2).max(1) as isize;
    match (key.code, key.modifiers) {
        (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => panel.move_selection(1),
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => panel.move_selection(-1),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => panel.move_selection(half),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => panel.move_selection(-half),
        (KeyCode::Char('g') | KeyCode::Home, KeyModifiers::NONE) => panel.selected = 0,
        (KeyCode::Char('G'), _) | (KeyCode::End, _) => {
            panel.selected = panel.items.len().saturating_sub(1)
        }
        (KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right, KeyModifiers::NONE) => {
            activate(editor, jobs, dock)
        }
        (KeyCode::Esc | KeyCode::Tab, KeyModifiers::NONE) => panel.focused = false,
        (KeyCode::Char('q'), KeyModifiers::NONE) => editor.close_panel(dock),
        _ => return false,
    }
    helix_event::request_redraw();
    true
}

/// A mouse event, if it is in one of the panels' `areas`. Clicks focus the
/// panel and select and run an item.
pub fn handle_mouse(
    editor: &mut Editor,
    jobs: &mut Jobs,
    event: &MouseEvent,
    areas: &[(Dock, Rect)],
) -> bool {
    let Some(&(dock, area)) = areas.iter().find(|(_, area)| {
        event.column >= area.x
            && event.column < area.right()
            && event.row >= area.y
            && event.row < area.bottom()
    }) else {
        return false;
    };
    if editor.panel(dock).is_none() {
        return false;
    }
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            editor.focus_panel(dock, true);
            let row = (event.row - area.y) as usize;
            if row == 0 {
                return true; // the title (or the bottom panel's border)
            }
            let panel = editor.panel_mut(dock).unwrap();
            let index = panel.scroll + row - 1;
            if index < panel.items.len() {
                panel.selected = index;
                activate(editor, jobs, dock);
            }
        }
        MouseEventKind::ScrollDown => editor.panel_mut(dock).unwrap().move_selection(3),
        MouseEventKind::ScrollUp => editor.panel_mut(dock).unwrap().move_selection(-3),
        _ => {}
    }
    helix_event::request_redraw();
    true
}

/// `left`, `right` or `bottom`.
pub fn parse_dock(name: &str) -> Option<Dock> {
    match name {
        "left" => Some(Dock::Left),
        "right" => Some(Dock::Right),
        "bottom" => Some(Dock::Bottom),
        _ => None,
    }
}
