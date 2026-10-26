//! Paths plugins watch (API 0.5 `watch`): file system events, gathered for
//! a moment and delivered as one `file-change` event.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use helix_event::runtime_local;
use notify::Watcher;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::job;

/// Events within this long of the first are delivered together.
const GATHER: Duration = Duration::from_millis(100);
/// Most paths in one event; the rest are dropped (the plugin rescans).
const MAX_PATHS: usize = 1000;

runtime_local! {
    /// Watchers by `(plugin, request id)`; dropping one stops it.
    static WATCHERS: Mutex<BTreeMap<(String, u32), notify::RecommendedWatcher>> = Mutex::new(BTreeMap::new());
}

/// Watch `path` for `plugin`'s request `id`.
pub fn start(plugin: String, id: u32, path: PathBuf, recursive: bool) -> Result<(), String> {
    let (sender, mut receiver) = mpsc::unbounded_channel::<Vec<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            if !matches!(event.kind, notify::EventKind::Access(_)) {
                let _ = sender.send(event.paths);
            }
        }
    })
    .map_err(|err| format!("can't watch {}: {err}", path.display()))?;
    let mode = if recursive {
        notify::RecursiveMode::Recursive
    } else {
        notify::RecursiveMode::NonRecursive
    };
    watcher
        .watch(&path, mode)
        .map_err(|err| format!("can't watch {}: {err}", path.display()))?;
    WATCHERS.lock().insert((plugin.clone(), id), watcher);
    tokio::spawn(async move {
        // Ends when the watcher (and with it the sender) is dropped.
        while let Some(first) = receiver.recv().await {
            let mut paths: BTreeSet<PathBuf> = first.into_iter().collect();
            tokio::time::sleep(GATHER).await;
            while let Ok(more) = receiver.try_recv() {
                paths.extend(more);
            }
            let paths: Vec<String> = paths
                .into_iter()
                .take(MAX_PATHS)
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            let plugin = plugin.clone();
            job::dispatch_callback(job::Callback::EditorCompositorJobs(Box::new(
                move |editor, _, jobs| {
                    let snapshot = super::snapshot(editor);
                    match super::with_host(|host| host.file_change(&plugin, id, paths, &snapshot))
                        .flatten()
                    {
                        Some(Ok(actions)) => super::apply(editor, Some(jobs), actions),
                        Some(Err(err)) => {
                            super::refresh_wanted(editor);
                            editor.set_error(err.to_string());
                        }
                        // It no longer watches.
                        None => stop(&plugin, id),
                    }
                },
            )))
            .await;
        }
    });
    Ok(())
}

pub fn stop(plugin: &str, id: u32) {
    WATCHERS.lock().remove(&(plugin.to_string(), id));
}

/// Stop `plugin`'s watchers, or everyone's.
pub fn stop_plugin(plugin: Option<&str>) {
    WATCHERS
        .lock()
        .retain(|(owner, _), _| plugin.is_some_and(|plugin| plugin != owner));
}
