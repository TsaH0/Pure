//! API 0.4 adapters: 0.3's, plus file operations with results, the
//! sidebar, panels, requests, revisions, snippets and listeners. API 0.5
//! has all of them too ([`v04_adapters`]).
use super::v0_4;
use super::world::world_adapters;

world_adapters!(v0_4, files_done);
v04_adapters!(v0_4);

/// The 0.4 imports for world module `$w` (0.4 or later), and the
/// `files_done` adapter `world_adapters!` needs.
macro_rules! v04_adapters {
    ($w:ident) => {
        use $w::helix::plugin::files;

        async fn files_done(
            instance: &$w::HelixPlugin,
            store: &mut Store<State>,
            id: u32,
            results: &[std::result::Result<(), String>],
        ) -> Result<()> {
            instance
                .helix_plugin_callbacks()
                .call_on_files_done(store, id, results)
                .await
        }

        impl files::Host for State {
            fn roots(&mut self) -> Vec<String> {
                State::file_roots(self)
            }
            fn apply(&mut self, ops: Vec<files::FileOp>) -> std::result::Result<u32, String> {
                use crate::FileOp;
                let ops = ops
                    .into_iter()
                    .map(|op| match op {
                        files::FileOp::Create(op) => FileOp::Create {
                            path: op.path,
                            directory: op.directory,
                        },
                        files::FileOp::Rename(op) => FileOp::Rename {
                            from: op.from,
                            to: op.to,
                        },
                        files::FileOp::Copy(op) => FileOp::Copy {
                            from: op.from,
                            to: op.to,
                        },
                        files::FileOp::Delete(path) => FileOp::Delete(path),
                        files::FileOp::Trash(path) => FileOp::Trash(path),
                    })
                    .collect();
                State::apply_files(self, ops)
            }
        }

        impl $w::helix::plugin::sidebar::Host for State {
            fn show(
                &mut self,
                view: $w::helix::plugin::sidebar::SidebarView,
            ) -> std::result::Result<(), String> {
                let items = view
                    .items
                    .into_iter()
                    .map(|item| crate::SidebarItem {
                        text: item.text,
                        scope: item.scope,
                        depth: item.depth.min(u16::MAX as u32) as u16,
                        command: item.command.map(|command| (command, item.args)),
                    })
                    .collect();
                State::show_sidebar(
                    self,
                    crate::Dock::Left,
                    view.title,
                    items,
                    view.width,
                    view.selected,
                )
            }
            fn close(&mut self) {
                State::close_sidebar(self, crate::Dock::Left)
            }
            fn focus(&mut self, focused: bool) {
                State::focus_sidebar(self, crate::Dock::Left, focused)
            }
            fn selected(&mut self) -> Option<u32> {
                State::sidebar_selected(self, crate::Dock::Left)
            }
        }

        fn dock(dock: $w::helix::plugin::panels::Dock) -> crate::Dock {
            use $w::helix::plugin::panels::Dock as D;
            match dock {
                D::Left => crate::Dock::Left,
                D::Right => crate::Dock::Right,
                D::Bottom => crate::Dock::Bottom,
            }
        }

        impl $w::helix::plugin::panels::Host for State {
            fn show(
                &mut self,
                panel: $w::helix::plugin::panels::Panel,
            ) -> std::result::Result<(), String> {
                let items = panel
                    .items
                    .into_iter()
                    .map(|item| crate::SidebarItem {
                        text: item.text,
                        scope: item.scope,
                        depth: item.depth.min(u16::MAX as u32) as u16,
                        command: item.command.map(|command| (command, item.args)),
                    })
                    .collect();
                State::show_sidebar(
                    self,
                    dock(panel.dock),
                    panel.title,
                    items,
                    panel.size,
                    panel.selected,
                )
            }
            fn close(&mut self, at: $w::helix::plugin::panels::Dock) {
                State::close_sidebar(self, dock(at))
            }
            fn focus(&mut self, at: $w::helix::plugin::panels::Dock, focused: bool) {
                State::focus_sidebar(self, dock(at), focused)
            }
            fn selected(&mut self, at: $w::helix::plugin::panels::Dock) -> Option<u32> {
                State::sidebar_selected(self, dock(at))
            }
        }

        impl $w::helix::plugin::requests::Host for State {
            fn capabilities(&mut self) -> Vec<String> {
                let v05 = stringify!($w) == "v0_5";
                crate::CAPABILITIES
                    .iter()
                    .chain(if v05 { crate::CAPABILITIES_05 } else { &[] })
                    .map(|c| c.to_string())
                    .collect()
            }
            fn cancel(&mut self, id: u32) {
                State::cancel_request(self, id)
            }
        }

        impl $w::helix::plugin::revisions::Host for State {
            fn revision(&mut self, doc: u32) -> std::result::Result<u64, String> {
                State::document_revision(self, doc)
            }
            fn apply_edits_at(
                &mut self,
                doc: u32,
                revision: u64,
                edits: Vec<$w::helix::plugin::types::Edit>,
            ) -> std::result::Result<(), String> {
                let edits = edits
                    .into_iter()
                    .map(|edit| crate::types::Edit {
                        from: edit.from,
                        to: edit.to,
                        text: edit.text,
                    })
                    .collect();
                State::apply_edits_at(self, doc, revision, edits)
            }
        }

        impl $w::helix::plugin::snippets::Host for State {
            fn complete(
                &mut self,
                id: u32,
                items: Vec<$w::helix::plugin::completion::CompletionItem>,
            ) -> std::result::Result<(), String> {
                let items = items
                    .into_iter()
                    .map(|item| crate::PluginCompletion {
                        label: item.label,
                        insert: item.insert,
                        kind: item.kind,
                        documentation: item.documentation,
                        replace_from: item.replace_from.map(|from| from as usize),
                        snippet: true,
                    })
                    .collect();
                State::complete(self, id, items)
            }
        }

        impl $w::helix::plugin::listener::Host for State {
            fn listen(&mut self, port: u16) -> std::result::Result<u32, String> {
                State::listen(self, port)
            }
        }
    };
}
pub(super) use v04_adapters;
