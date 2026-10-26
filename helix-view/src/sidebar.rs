//! Plugin panels (Pure): lists docked left, right or below the views, or
//! floating over them, e.g. a file tree, an outline, test results. One
//! panel per dock; the plugin
//! sets its items, and enter on an item runs the plugin command it names.
//! The sidebar of API 0.4 is the left panel.

/// Where a panel is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dock {
    Left,
    Right,
    Bottom,
    /// Floating over the editor.
    Float,
}

/// Where a float is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FloatPosition {
    Center,
    /// Below (or above) the cursor.
    Cursor,
    TopRight,
}

/// One row of a sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarItem {
    pub text: String,
    /// Theme scope of the text.
    pub scope: String,
    /// Indentation (tree depth), two columns each.
    pub depth: u16,
    /// `(command, arguments)`: one of the owner's commands, run with enter.
    pub command: Option<(String, Vec<String>)>,
}

#[derive(Debug, Clone)]
pub struct Sidebar {
    pub dock: Dock,
    /// The plugin showing it.
    pub owner: String,
    pub title: String,
    pub items: Vec<SidebarItem>,
    /// Columns (left, right) or rows (bottom), including the border.
    pub width: u16,
    pub selected: usize,
    /// The first item shown.
    pub scroll: usize,
    /// Keys go to the sidebar instead of the editor.
    pub focused: bool,
    /// A float's height (rows, with the border) and position.
    pub float: Option<(u16, FloatPosition)>,
}

/// Narrowest and widest side panel.
pub const MIN_WIDTH: u16 = 12;
pub const MAX_WIDTH: u16 = 80;
/// Lowest and highest bottom panel, in rows.
pub const MIN_HEIGHT: u16 = 3;
pub const MAX_HEIGHT: u16 = 40;

impl Sidebar {
    /// Move the selection by `delta` items, clamped.
    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.items.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    /// Keep the selection within `height` visible rows.
    pub fn scroll_to_selection(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
        let max_scroll = self.items.len().saturating_sub(height);
        self.scroll = self.scroll.min(max_scroll);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn sidebar(items: usize) -> Sidebar {
        Sidebar {
            dock: Dock::Left,
            owner: "p".into(),
            title: "t".into(),
            items: (0..items)
                .map(|i| SidebarItem {
                    text: i.to_string(),
                    scope: String::new(),
                    depth: 0,
                    command: None,
                })
                .collect(),
            width: 30,
            selected: 0,
            scroll: 0,
            focused: false,
            float: None,
        }
    }

    #[test]
    fn selection_stays_in_range_and_in_view() {
        let mut s = sidebar(10);
        s.move_selection(-1);
        assert_eq!(s.selected, 0);
        s.move_selection(25);
        assert_eq!(s.selected, 9);
        s.scroll_to_selection(4);
        assert_eq!(s.scroll, 6);
        s.move_selection(-8);
        s.scroll_to_selection(4);
        assert_eq!((s.selected, s.scroll), (1, 1));
        let mut empty = sidebar(0);
        empty.move_selection(3);
        assert_eq!(empty.selected, 0);
    }
}
