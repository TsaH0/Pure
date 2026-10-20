//! API 0.3 adapters. Older worlds retain their generated types unchanged.
use super::v0_3;
use super::world::world_adapters;

world_adapters!(v0_3, files_done);

/// 0.3 has no file results (`files.apply` is 0.4).
async fn files_done(
    _instance: &v0_3::HelixPlugin,
    _store: &mut Store<State>,
    _id: u32,
    _results: &[std::result::Result<(), String>],
) -> Result<()> {
    anyhow::bail!("file results require API 0.4")
}

impl v0_3::helix::plugin::files::Host for State {
    fn roots(&mut self) -> Vec<String> {
        State::file_roots(self)
    }
    fn create_path(&mut self, path: String, directory: bool) -> std::result::Result<(), String> {
        State::create_path(self, path, directory)
    }
    fn rename_path(&mut self, from: String, to: String) -> std::result::Result<(), String> {
        State::rename_path(self, from, to)
    }
    fn delete_path(&mut self, path: String) -> std::result::Result<(), String> {
        State::delete_path(self, path)
    }
}
