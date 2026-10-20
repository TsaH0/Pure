//! API 0.4 adapters: 0.3's, plus file operations with results.
use super::v0_4;
use super::world::world_adapters;
use v0_4::helix::plugin::files;

world_adapters!(v0_4, files_done);

async fn files_done(
    instance: &v0_4::HelixPlugin,
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
