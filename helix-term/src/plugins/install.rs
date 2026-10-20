//! `:plugin-install`, `:plugin-update`, `:plugin-remove` and `plugins.lock`.
//!
//! A plugin is installed from a git repository or a local directory that has
//! a `plugin.toml` and the committed component it names (nothing is built on
//! the user's machine). Only those two files are copied to
//! `<plugins>/<name>/`. `plugins.lock` records where each installed plugin
//! came from, its commit, and the SHA-256 of its component: a component that
//! changed since is refused until it is updated or installed again. Plugins
//! copied in by hand aren't in the lock and load as before.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{anyhow, bail, Context as _};
use helix_plugin::Manifest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockEntry {
    /// A git URL or a local directory.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub sha256: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Lock(pub BTreeMap<String, LockEntry>);

impl Lock {
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(source) => Ok(Self(
                toml::from_str(&source).with_context(|| format!("invalid {}", path.display()))?,
            )),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let source = format!(
            "# Written by :plugin-install; checked when plugins load.\n{}",
            toml::to_string(&self.0)?
        );
        write_atomically(path, source.as_bytes())
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let tmp = path.with_extension(format!(
        "tmp{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Locked plugins in `plugins` whose component changed since: `(name,
/// reason)`. They must not be loaded.
pub fn verify(plugins: &Path, lock: &Lock) -> Vec<(String, String)> {
    let mut changed = Vec::new();
    for (name, entry) in &lock.0 {
        let dir = plugins.join(name);
        let Ok(manifest) = read_manifest(&dir) else {
            continue; // removed by hand: nothing to load
        };
        match sha256_file(&dir.join(&manifest.wasm)) {
            Ok(hash) if hash == entry.sha256 => {}
            Ok(_) => changed.push((
                name.clone(),
                format!(
                    "plugin '{name}' changed since it was installed (its component's \
                     SHA-256 doesn't match plugins.lock): run :plugin-update {name} or \
                     :plugin-install it again"
                ),
            )),
            Err(err) => changed.push((name.clone(), format!("{err:#}"))),
        }
    }
    changed
}

fn read_manifest(dir: &Path) -> anyhow::Result<Manifest> {
    let path = dir.join("plugin.toml");
    let source = std::fs::read_to_string(&path)
        .with_context(|| format!("no plugin.toml in {}", dir.display()))?;
    Manifest::parse(&source).with_context(|| format!("invalid {}", path.display()))
}

/// Run git without asking for credentials on the terminal (the editor owns
/// it), stopping it after `GIT_TIMEOUT`.
fn git(command: &mut Command) -> anyhow::Result<std::process::Output> {
    const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
    let mut child = command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to run git")?;
    let deadline = std::time::Instant::now() + GIT_TIMEOUT;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("git took longer than {}s", GIT_TIMEOUT.as_secs());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn is_git(source: &str) -> bool {
    source.contains("://") || source.starts_with("git@") || source.ends_with(".git")
}

/// A plugin fetched into a temporary directory, waiting for the user's
/// consent. Dropping it removes the directory.
#[derive(Debug)]
pub struct Staged {
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub source: String,
    pub commit: Option<String>,
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Fetch `source` (a git URL or a directory) into a temporary directory
/// below `plugins`. Blocking: run it off the editor's thread.
pub fn stage(plugins: &Path, source: &str) -> anyhow::Result<Staged> {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    std::fs::create_dir_all(plugins)?;
    let dir = plugins.join(format!(
        ".staging-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let fetched = dir.join("source");
    // Remove `dir` again on any error below.
    let mut staged = Staged {
        dir,
        manifest: Manifest::parse("name = \"x\"\nversion = \"0\"\napi = \"0.1\"")?,
        source: source.to_string(),
        commit: None,
    };
    std::fs::create_dir_all(&staged.dir)?;
    if is_git(source) {
        let output = git(
            Command::new("git")
                .args(["clone", "--depth", "1", "--quiet", "--", source])
                .arg(&fetched),
        )?;
        if !output.status.success() {
            bail!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let output = git(Command::new("git")
            .arg("-C")
            .arg(&fetched)
            .args(["rev-parse", "HEAD"]))?;
        staged.commit = Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
    } else {
        let path = helix_stdx::path::expand_tilde(Path::new(source));
        let path = helix_stdx::path::canonicalize(path);
        if !path.is_dir() {
            bail!("{} is not a directory or git URL", path.display());
        }
        staged.source = path.to_string_lossy().into_owned();
        std::fs::create_dir_all(&fetched)?;
        let manifest = read_manifest(&path)?;
        for file in [PathBuf::from("plugin.toml"), manifest.wasm.clone()] {
            std::fs::copy(path.join(&file), fetched.join(&file))
                .with_context(|| format!("failed to copy {}", file.display()))?;
        }
    }
    let manifest = read_manifest(&fetched)?;
    if manifest.wasm.is_absolute() || manifest.wasm.components().count() != 1 {
        bail!("plugin.toml's wasm must be a file next to it");
    }
    if !fetched.join(&manifest.wasm).is_file() {
        bail!(
            "the plugin has no committed {} (it must be built and committed; \
             nothing is built on install)",
            manifest.wasm.display()
        );
    }
    staged.manifest = manifest;
    Ok(staged)
}

/// Install a staged plugin into `plugins/<name>` (replacing an older copy)
/// and record it in the lock at `lock_path`.
pub fn commit(plugins: &Path, lock_path: &Path, staged: Staged) -> anyhow::Result<String> {
    let name = staged.manifest.name.clone();
    let fetched = staged.dir.join("source");
    let target = plugins.join(&name);
    let wasm = staged.manifest.wasm.clone();
    let sha256 = sha256_file(&fetched.join(&wasm))?;
    // Prepare next to the target, then swap, so a failure leaves the old copy.
    let fresh = staged.dir.join("install");
    std::fs::create_dir_all(&fresh)?;
    for file in [PathBuf::from("plugin.toml"), wasm] {
        std::fs::copy(fetched.join(&file), fresh.join(&file))?;
    }
    // Keep the old copy until the new one and the lock are both in place:
    // on any failure it is put back (the staging directory is removed with
    // `staged` either way).
    let mut lock = Lock::read(lock_path)?;
    let old = staged.dir.join("old");
    let replacing = target.exists();
    if replacing {
        std::fs::rename(&target, &old)
            .with_context(|| format!("failed to replace {}", target.display()))?;
    }
    let restore = |err: anyhow::Error| {
        if replacing {
            let _ = std::fs::remove_dir_all(&target);
            let _ = std::fs::rename(&old, &target);
        }
        err
    };
    std::fs::rename(&fresh, &target).map_err(|err| restore(err.into()))?;
    lock.0.insert(
        name.clone(),
        LockEntry {
            source: staged.source.clone(),
            commit: staged.commit.clone(),
            sha256,
        },
    );
    lock.write(lock_path).map_err(restore)?;
    Ok(name)
}

/// Remove plugin `name` and its lock entry.
pub fn remove(plugins: &Path, lock_path: &Path, name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        bail!("invalid plugin name '{name}'");
    }
    let dir = plugins.join(name);
    if !dir.join("plugin.toml").is_file() {
        bail!("no plugin named '{name}' in {}", plugins.display());
    }
    std::fs::remove_dir_all(&dir)?;
    let mut lock = Lock::read(lock_path)?;
    if lock.0.remove(name).is_some() {
        lock.write(lock_path)?;
    }
    Ok(())
}

/// Fetch the newest version of installed plugin `name` from where it came
/// from. Returns it staged with its current manifest, so the caller can ask
/// again if the permissions grew.
pub fn stage_update(
    plugins: &Path,
    lock_path: &Path,
    name: &str,
) -> anyhow::Result<(Staged, Manifest)> {
    let lock = Lock::read(lock_path)?;
    let entry = lock
        .0
        .get(name)
        .ok_or_else(|| anyhow!("'{name}' wasn't installed with :plugin-install"))?;
    let current = read_manifest(&plugins.join(name))?;
    let staged = stage(plugins, &entry.source)?;
    if staged.manifest.name != name {
        bail!(
            "{} now has a plugin named '{}', not '{name}'",
            entry.source,
            staged.manifest.name
        );
    }
    Ok((staged, current))
}

/// Permissions in `new` that `old` didn't ask for.
pub fn new_permissions(old: &Manifest, new: &Manifest) -> Vec<&'static str> {
    new.permissions
        .iter()
        .filter(|permission| !old.permissions.contains(permission))
        .map(|permission| permission.as_str())
        .collect()
}

// -- The editor commands. --

use helix_view::Editor;

use crate::compositor::Compositor;
use crate::job;
use crate::ui::{Prompt, PromptEvent};

fn paths() -> (PathBuf, PathBuf) {
    let plugins = super::plugin_dir();
    let lock = super::lock_file(&plugins);
    (plugins, lock)
}

/// Ask `question` (y/n) and run `yes` with the editor if the answer is yes.
fn confirm(compositor: &mut Compositor, question: String, yes: impl FnOnce(&mut Editor) + 'static) {
    let mut yes = Some(yes);
    let prompt = Prompt::new(
        format!("{} (y/n): ", question.trim_end()).into(),
        None,
        |_, _| Vec::new(),
        move |cx, input, event| {
            if event != PromptEvent::Validate {
                return;
            }
            match input.trim() {
                "y" | "Y" | "yes" => {
                    if let Some(yes) = yes.take() {
                        yes(cx.editor);
                    }
                }
                _ => cx.editor.set_status("Cancelled"),
            }
        },
    );
    compositor.push(Box::new(prompt));
}

fn describe_permissions(manifest: &Manifest) -> String {
    if manifest.permissions.is_empty() {
        "no permissions".into()
    } else {
        let list: Vec<_> = manifest.permissions.iter().map(|p| p.as_str()).collect();
        format!("permissions: {}", list.join(", "))
    }
}

/// `:plugin-install <git url | directory>`: fetch in the background, then
/// show what the plugin asks for and install it if the user agrees.
pub fn install_command(editor: &mut Editor, source: String) {
    editor.set_status(format!("Fetching {source}..."));
    tokio::spawn(async move {
        let (plugins, _) = paths();
        let staged = tokio::task::spawn_blocking(move || stage(&plugins, &source)).await;
        job::dispatch(move |editor, compositor| {
            let staged = match staged {
                Ok(Ok(staged)) => staged,
                Ok(Err(err)) => return editor.set_error(format!("{err:#}")),
                Err(err) => return editor.set_error(err.to_string()),
            };
            let manifest = &staged.manifest;
            let (plugins, _) = paths();
            if plugins.join(&manifest.name).exists() {
                return editor.set_error(format!(
                    "'{}' is already installed: use :plugin-update {0}",
                    manifest.name
                ));
            }
            let question = format!(
                "Install {} {} ({})? {}",
                manifest.name,
                manifest.version,
                describe_permissions(manifest),
                if manifest.permissions.iter().any(|p| {
                    matches!(
                        p,
                        helix_plugin::Permission::Process | helix_plugin::Permission::Filesystem
                    )
                }) {
                    "Programs and files also need allow-process / allow-files in plugins.toml."
                } else {
                    ""
                }
            );
            confirm(compositor, question, move |editor| {
                let (plugins, lock) = paths();
                match commit(&plugins, &lock, staged) {
                    Ok(name) => {
                        super::load(editor);
                        if editor.get_status().is_none_or(|(_, severity)| {
                            *severity != helix_view::editor::Severity::Error
                        }) {
                            editor.set_status(format!("Installed {name}"));
                        }
                    }
                    Err(err) => editor.set_error(format!("{err:#}")),
                }
            });
        })
        .await;
    });
}

/// `:plugin-update [names]`: fetch installed plugins again; ask before
/// giving one new permissions.
pub fn update_command(editor: &mut Editor, names: Vec<String>) {
    let names = if names.is_empty() {
        let (_, lock) = paths();
        match Lock::read(&lock) {
            Ok(lock) => lock.0.keys().cloned().collect(),
            Err(err) => return editor.set_error(format!("{err:#}")),
        }
    } else {
        names
    };
    if names.is_empty() {
        return editor.set_status("No plugins were installed with :plugin-install");
    }
    editor.set_status(format!("Updating {}...", names.join(", ")));
    tokio::spawn(async move {
        let fetched = tokio::task::spawn_blocking(move || {
            let (plugins, lock) = paths();
            names
                .into_iter()
                .map(|name| {
                    let staged = stage_update(&plugins, &lock, &name);
                    (name, staged)
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        job::dispatch(move |editor, compositor| {
            let (plugins, lock) = paths();
            let mut updated = Vec::new();
            let mut asks = Vec::new();
            for (name, staged) in fetched {
                match staged {
                    Ok((staged, current)) => {
                        let added = new_permissions(&current, &staged.manifest);
                        if added.is_empty() {
                            match commit(&plugins, &lock, staged) {
                                Ok(name) => updated.push(name),
                                Err(err) => editor.set_error(format!("{name}: {err:#}")),
                            }
                        } else {
                            asks.push((name, added.join(", "), staged));
                        }
                    }
                    Err(err) => editor.set_error(format!("{name}: {err:#}")),
                }
            }
            if !updated.is_empty() {
                super::load(editor);
                editor.set_status(format!("Updated {}", updated.join(", ")));
            }
            // New permissions need consent, one plugin at a time.
            for (name, added, staged) in asks {
                confirm(
                    compositor,
                    format!("The new {name} also asks for: {added}. Update?"),
                    move |editor| {
                        let (plugins, lock) = paths();
                        match commit(&plugins, &lock, staged) {
                            Ok(name) => {
                                super::load(editor);
                                editor.set_status(format!("Updated {name}"));
                            }
                            Err(err) => editor.set_error(format!("{err:#}")),
                        }
                    },
                );
            }
        })
        .await;
    });
}

/// `:plugin-remove <name>`, after asking.
pub fn remove_command(compositor: &mut Compositor, name: String) {
    confirm(
        compositor,
        format!("Remove plugin {name}?"),
        move |editor| {
            let (plugins, lock) = paths();
            match remove(&plugins, &lock, &name) {
                Ok(()) => {
                    super::load(editor);
                    editor.set_status(format!("Removed {name}"));
                }
                Err(err) => editor.set_error(format!("{err:#}")),
            }
        },
    );
}

#[cfg(test)]
mod test {
    use super::*;

    fn todos() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../helix-plugin/examples/todos")
    }

    #[test]
    fn install_update_and_remove_from_a_directory() {
        let root = tempfile::tempdir().unwrap();
        let plugins = root.path().join("plugins");
        let lock_path = root.path().join("plugins.lock");

        let staged = stage(&plugins, todos().to_str().unwrap()).unwrap();
        assert_eq!(staged.manifest.name, "todos");
        assert_eq!(staged.commit, None);
        let staging = staged.dir.clone();
        let name = commit(&plugins, &lock_path, staged).unwrap();
        assert_eq!(name, "todos");
        // Only the manifest and the component; staging is gone.
        let mut files: Vec<_> = std::fs::read_dir(plugins.join("todos"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        assert_eq!(files, ["plugin.toml", "plugin.wasm"]);
        assert!(!staging.exists());

        let lock = Lock::read(&lock_path).unwrap();
        let expected = sha256_file(&todos().join("plugin.wasm")).unwrap();
        assert_eq!(lock.0["todos"].sha256, expected);
        assert!(verify(&plugins, &lock).is_empty());

        // A changed component is refused until updated.
        std::fs::write(plugins.join("todos/plugin.wasm"), b"tampered").unwrap();
        let changed = verify(&plugins, &lock);
        assert_eq!(changed.len(), 1);
        assert!(changed[0].1.contains("changed since it was installed"));
        let (staged, current) = stage_update(&plugins, &lock_path, "todos").unwrap();
        assert!(new_permissions(&current, &staged.manifest).is_empty());
        commit(&plugins, &lock_path, staged).unwrap();
        assert!(verify(&plugins, &Lock::read(&lock_path).unwrap()).is_empty());

        remove(&plugins, &lock_path, "todos").unwrap();
        assert!(!plugins.join("todos").exists());
        assert!(Lock::read(&lock_path).unwrap().0.is_empty());
        assert!(remove(&plugins, &lock_path, "../etc").is_err());
    }

    #[test]
    fn install_from_git_records_the_commit() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for file in ["plugin.toml", "plugin.wasm"] {
            std::fs::copy(todos().join(file), repo.join(file)).unwrap();
        }
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(status.status.success(), "{status:?}");
            String::from_utf8(status.stdout).unwrap()
        };
        git(&["init", "--quiet"]);
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "plugin"]);
        let head = git(&["rev-parse", "HEAD"]);
        let plugins = root.path().join("plugins");
        let url = format!("file://{}", repo.display());
        let staged = stage(&plugins, &url).unwrap();
        assert_eq!(staged.commit.as_deref(), Some(head.trim()));
        commit(&plugins, &root.path().join("plugins.lock"), staged).unwrap();
        assert!(plugins.join("todos/plugin.wasm").is_file());
    }

    #[test]
    fn plugins_without_a_component_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::copy(todos().join("plugin.toml"), source.join("plugin.toml")).unwrap();
        let err = stage(&root.path().join("plugins"), source.to_str().unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("failed to copy"), "{err:#}");
        // Nothing left behind.
        let left: Vec<_> = std::fs::read_dir(root.path().join("plugins"))
            .unwrap()
            .collect();
        assert!(left.is_empty());
    }

    #[test]
    fn components_outside_the_plugin_are_refused_before_copying() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("a/b/source");
        std::fs::create_dir_all(&source).unwrap();
        let victim = root.path().join("a/victim.wasm");
        std::fs::write(&victim, b"keep").unwrap();
        std::fs::write(
            source.join("plugin.toml"),
            "name = \"evil\"\nversion = \"1\"\napi = \"0.1\"\nwasm = \"../../victim.wasm\"",
        )
        .unwrap();
        let plugins = root.path().join("a/b/plugins");
        let err = stage(&plugins, source.to_str().unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("wasm must be a file next to"), "{err:#}");
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
    }

    #[test]
    fn a_failed_update_keeps_the_old_copy() {
        let root = tempfile::tempdir().unwrap();
        let plugins = root.path().join("plugins");
        let lock = root.path().join("plugins.lock");
        let staged = stage(&plugins, todos().to_str().unwrap()).unwrap();
        commit(&plugins, &lock, staged).unwrap();
        let before = std::fs::read(plugins.join("todos/plugin.wasm")).unwrap();

        // The lock can't be written (its temporary files are directories, as
        // root can write anywhere): the update is undone.
        let staged = stage(&plugins, todos().to_str().unwrap()).unwrap();
        for n in 0..1000 {
            let tmp = format!("plugins.tmp{}-{n}", std::process::id());
            std::fs::create_dir_all(root.path().join(tmp)).unwrap();
        }
        let err = commit(&plugins, &lock, staged).unwrap_err();
        assert!(format!("{err:#}").contains("directory"), "{err:#}");
        assert_eq!(std::fs::read(plugins.join("todos/plugin.wasm")).unwrap(), before);
        assert!(plugins.join("todos/plugin.toml").is_file());
        let left: Vec<_> = std::fs::read_dir(&plugins)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["todos"]);
    }
}
