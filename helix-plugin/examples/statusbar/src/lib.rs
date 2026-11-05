//! Example plugin (API 0.5 `bars`): a statusline like lualine (sections in
//! the mode's colours) and a tab line of the open buffers, like
//! bufferline.nvim. The statusline is set once: the editor fills in the
//! mode, position and the rest when it draws it. The tab line is redrawn
//! when buffers open, close or get the focus.

use std::cell::Cell;

use helix_plugin_api::{
    bars::{self, span, Bar, Span},
    editor, register_plugin,
    subscriptions::{self, EventType},
    Event, Plugin, Registration,
};

thread_local! {
    static TABLINE: Cell<bool> = const { Cell::new(true) };
}

fn statusline() -> Bar {
    Bar {
        left: vec![
            span(" %{mode} ", "mode"),
            span(" %{branch} ", "ui.statusline.inactive"),
            span(" %{file}%{modified} ", ""),
        ],
        center: vec![],
        right: vec![
            span(" %{errors}✗ %{warnings}⚠ ", "warning"),
            span(" %{language} ", ""),
            span(" %{percent} ", "ui.statusline.inactive"),
            span(" %{line}:%{column} ", "mode"),
        ],
    }
}

/// The name a buffer goes by in the tab line.
fn name(path: Option<String>) -> String {
    path.as_deref()
        .and_then(|path| path.rsplit('/').next())
        .map_or_else(|| "[scratch]".to_string(), str::to_string)
}

/// One tab per buffer; the current one stands out.
fn tabs(buffers: &[(u32, Option<String>)], current: Option<u32>) -> Vec<Span> {
    buffers
        .iter()
        .map(|(doc, path)| {
            let scope = if Some(*doc) == current {
                "ui.bufferline.active"
            } else {
                "ui.bufferline"
            };
            span(&format!(" {} ", name(path.clone())), scope)
        })
        .collect()
}

fn draw_tabline() {
    if !TABLINE.get() {
        let _ = bars::set_tabline(None);
        return;
    }
    let buffers: Vec<_> = editor::documents()
        .into_iter()
        .map(|doc| (doc, editor::document_path(doc).ok().flatten()))
        .collect();
    let bar = Bar {
        left: tabs(&buffers, editor::current_document()),
        center: vec![],
        right: vec![span(" %{buffers} buffers ", "ui.statusline.inactive")],
    };
    if let Err(err) = bars::set_tabline(Some(&bar)) {
        editor::log(&err);
    }
}

fn configure(config: &str) {
    let config: serde_json::Value = serde_json::from_str(config).unwrap_or_default();
    TABLINE.set(config["tabline"].as_bool().unwrap_or(true));
}

struct Statusbar;

impl Plugin for Statusbar {
    fn init(config: String) -> Result<Registration, String> {
        configure(&config);
        bars::set_statusline(None, Some(&statusline()))?;
        let any = subscriptions::any();
        for event in [EventType::DocumentOpen, EventType::DocumentClose, EventType::BufferEnter, EventType::ConfigChange] {
            subscriptions::subscribe(event, &any)?;
        }
        draw_tabline();
        Ok(Registration {
            commands: vec![],
            events: vec![],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        Err(format!("unknown command {name}"))
    }

    fn on_document_open(_doc: u32) {
        draw_tabline();
    }

    fn on_document_close(_doc: u32) {
        draw_tabline();
    }

    fn on_event(event: Event) {
        match event {
            Event::ConfigChange(config) => {
                configure(&config);
                draw_tabline();
            }
            Event::BufferEnter(_) => draw_tabline(),
            _ => {}
        }
    }
}

register_plugin!(Statusbar);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn tabs_mark_the_current_buffer() {
        let buffers = [(1, Some("src/main.rs".to_string())), (2, None)];
        let tabs = tabs(&buffers, Some(2));
        assert_eq!(tabs[0].text, " main.rs ");
        assert_eq!(tabs[0].scope, "ui.bufferline");
        assert_eq!(tabs[1].text, " [scratch] ");
        assert_eq!(tabs[1].scope, "ui.bufferline.active");
    }
}
