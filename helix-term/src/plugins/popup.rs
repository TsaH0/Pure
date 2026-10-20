//! Plugin-owned popup lifecycle. Ordinary 0.2 popups keep their original widget.
use crate::compositor::{Component, Compositor, Context, Event, EventResult};
use crate::ui::{Markdown, Popup};
use helix_plugin::Callback;
use helix_view::{
    graphics::Rect,
    input::{KeyCode, KeyModifiers},
    Editor,
};
use tui::buffer::Buffer as Surface;

struct Contents {
    markdown: Markdown,
    owner: (String, u32),
}
impl Component for Contents {
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        self.markdown.render(area, surface, cx)
    }
    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        self.markdown.required_size(viewport)
    }
}
impl Drop for Contents {
    fn drop(&mut self) {
        if tokio::runtime::Handle::try_current().is_ok() {
            super::deliver_later(
                self.owner.0.clone(),
                Callback::PopupClosed { id: self.owner.1 },
            );
        }
    }
}

struct PluginPopup {
    popup: Popup<Contents>,
}
impl Component for PluginPopup {
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        self.popup.render(area, surface, cx)
    }
    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        self.popup.required_size(viewport)
    }
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if let Event::Key(key) = event {
            let scroll = matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
                || (key.modifiers == KeyModifiers::CONTROL
                    && matches!(key.code, KeyCode::Char('u' | 'd')));
            if !scroll {
                let owner = self.popup.contents().owner.clone();
                let callback = Box::new(move |compositor: &mut Compositor, _: &mut Context| {
                    close(compositor, &owner.0, owner.1)
                });
                if matches!(key.code, KeyCode::Esc)
                    || (key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c'))
                {
                    return EventResult::Consumed(Some(callback));
                }
                return EventResult::Ignored(Some(callback));
            }
        }
        self.popup.handle_event(event, cx)
    }
}

pub fn show(
    editor: &mut Editor,
    compositor: &mut Compositor,
    markdown: String,
    centered: bool,
    owner: Option<(String, u32)>,
) {
    let markdown = Markdown::new(markdown, editor.syn_loader.clone());
    if let Some(owner) = owner {
        let popup =
            Popup::new("plugin-popup-owned", Contents { markdown, owner }).centered(centered);
        compositor.push(Box::new(PluginPopup { popup }));
    } else {
        compositor.replace_or_push(
            "plugin-popup",
            Popup::new("plugin-popup", markdown).auto_close(true),
        );
    }
}

pub fn close(compositor: &mut Compositor, plugin: &str, id: u32) {
    compositor.remove_matching::<PluginPopup>(|popup| {
        let owner = &popup.popup.contents().owner;
        owner.0 == plugin && owner.1 == id
    });
}
