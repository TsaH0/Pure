//! Bars plugins draw: in place of the statusline, a tab line at the top of
//! the screen, and a bar at the top of each view (a winbar).

use std::collections::HashMap;

use crate::ViewId;

/// Text in a style: a theme scope or a style written out ("#ff8800").
/// `%{name}` in the text is filled in when the bar is drawn (see
/// `helix_term::ui::bars`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub scope: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bar {
    pub left: Vec<Span>,
    pub center: Vec<Span>,
    pub right: Vec<Span>,
}

/// The bars plugins set, each with the plugin that set it.
#[derive(Debug, Default)]
pub struct PluginBars {
    pub statusline: Option<(String, Bar)>,
    pub statuslines: HashMap<ViewId, (String, Bar)>,
    pub tabline: Option<(String, Bar)>,
    pub winbar: Option<(String, Bar)>,
    pub winbars: HashMap<ViewId, (String, Bar)>,
}

impl PluginBars {
    pub fn statusline(&self, view: ViewId) -> Option<&Bar> {
        self.statuslines
            .get(&view)
            .or(self.statusline.as_ref())
            .map(|(_, bar)| bar)
    }

    pub fn winbar(&self, view: ViewId) -> Option<&Bar> {
        self.winbars
            .get(&view)
            .or(self.winbar.as_ref())
            .map(|(_, bar)| bar)
    }

    /// Whether any view has a winbar (they need their layout redone).
    pub fn any_winbar(&self) -> bool {
        self.winbar.is_some() || !self.winbars.is_empty()
    }

    /// Remove what `plugin` set (`None`: every plugin's).
    pub fn clear_plugin(&mut self, plugin: Option<&str>) {
        let theirs = |(owner, _): &(String, Bar)| plugin.is_none_or(|plugin| plugin == owner);
        if self.statusline.as_ref().is_some_and(theirs) {
            self.statusline = None;
        }
        if self.tabline.as_ref().is_some_and(theirs) {
            self.tabline = None;
        }
        if self.winbar.as_ref().is_some_and(theirs) {
            self.winbar = None;
        }
        self.statuslines.retain(|_, bar| !theirs(bar));
        self.winbars.retain(|_, bar| !theirs(bar));
    }
}
