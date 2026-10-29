//! Noticing files changed on disk (`editor.auto-reload`). The directories
//! holding open files are watched (a directory, not the file, so a save
//! that replaces the file is seen too); the editor then only wakes when
//! something there changes. Where watching fails it checks every second.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use helix_view::Editor;
use notify::Watcher;
use tokio::sync::mpsc;

/// Changes within this long of the first are handled together, so a file
/// written in pieces is reloaded once.
const GATHER: Duration = Duration::from_millis(50);

pub struct AutoReload {
    /// `None` when watching isn't possible: check on `timer` instead.
    watcher: Option<notify::RecommendedWatcher>,
    watched: HashSet<PathBuf>,
    /// Open documents when the watched set was last pruned.
    documents: usize,
    /// Changed paths, a batch at a time.
    changes: mpsc::UnboundedReceiver<Vec<PathBuf>>,
    timer: tokio::time::Interval,
}

impl AutoReload {
    pub fn new() -> Self {
        let (batches, changes) = mpsc::unbounded_channel();
        let (sender, mut receiver) = mpsc::unbounded_channel::<Vec<PathBuf>>();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event {
                if !matches!(event.kind, notify::EventKind::Access(_)) {
                    let _ = sender.send(event.paths);
                }
            }
        })
        .map_err(|err| log::warn!("auto-reload: can't watch files, checking each second: {err}"))
        .ok();
        if watcher.is_some() {
            tokio::spawn(async move {
                // Ends when the watcher (and with it the sender) is dropped.
                while let Some(mut paths) = receiver.recv().await {
                    tokio::time::sleep(GATHER).await;
                    while let Ok(more) = receiver.try_recv() {
                        paths.extend(more);
                    }
                    if batches.send(paths).is_err() {
                        break;
                    }
                }
            });
        }
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Self {
            watcher,
            watched: HashSet::new(),
            documents: 0,
            changes,
            timer,
        }
    }

    /// Watch the directories of `editor`'s open files. Cheap when nothing
    /// changed: called after every event.
    pub fn sync(&mut self, editor: &Editor) {
        let Some(watcher) = &mut self.watcher else {
            return;
        };
        if !editor.config().auto_reload {
            if !self.watched.is_empty() {
                for dir in self.watched.drain() {
                    let _ = watcher.unwatch(&dir);
                }
            }
            return;
        }
        let dirs = || {
            editor
                .documents()
                .filter_map(|doc| doc.path()?.parent())
                .filter(|dir| !dir.as_os_str().is_empty())
        };
        // Closed files' directories go when the number of files changes.
        if editor.documents.len() != self.documents {
            self.documents = editor.documents.len();
            let wanted: HashSet<&Path> = dirs().collect();
            self.watched.retain(|dir| {
                let keep = wanted.contains(dir.as_path());
                if !keep {
                    let _ = watcher.unwatch(dir);
                }
                keep
            });
        }
        let mut failed = false;
        for dir in dirs() {
            if self.watched.contains(dir) {
                continue;
            }
            match watcher.watch(dir, notify::RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.watched.insert(dir.to_path_buf());
                }
                Err(err) => {
                    log::warn!(
                        "auto-reload: can't watch {}, checking each second: {err}",
                        dir.display()
                    );
                    failed = true;
                    break;
                }
            }
        }
        if failed {
            // Out of watches, say: check every file on the timer instead.
            self.watcher = None;
            self.watched.clear();
        }
    }

    /// Whether checking needs the timer.
    pub fn polls(&self) -> bool {
        self.watcher.is_none()
    }

    /// The next changed paths, or nothing on the next tick when polling.
    /// Cancel safe.
    pub async fn changed(&mut self) -> Vec<PathBuf> {
        if self.watcher.is_none() {
            self.timer.tick().await;
            return Vec::new();
        }
        match self.changes.recv().await {
            Some(paths) => paths,
            None => std::future::pending().await,
        }
    }

    /// Whether `paths` from [`Self::changed`] touch an open file.
    pub fn touches(&self, editor: &Editor, paths: &[PathBuf]) -> bool {
        self.polls()
            || paths
                .iter()
                .any(|path| editor.documents().any(|doc| doc.path() == Some(path)))
    }
}
