//! `editor.auto-reload`: files changed on disk by other programs.

use std::time::{Duration, SystemTime};

use helix_term::application::Application;
use helix_view::editor::Severity;

use super::*;

/// Writes `text` to `path` with a modification time `secs` from now, so the
/// change is seen even on file systems with coarse timestamps.
fn change_on_disk(path: &std::path::Path, text: &str, secs: u64) {
    std::fs::write(path, text).unwrap();
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(secs))
        .unwrap();
}

fn text(app: &Application) -> String {
    let (_, doc) = helix_view::current_ref!(app.editor);
    doc.text().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn reloads_unmodified_and_warns_about_modified() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("one\ntwo\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;

    // Nothing changed: nothing happens.
    assert!(!app.editor.reload_changed_documents());

    // An unmodified buffer follows the file; the cursor stays valid.
    change_on_disk(file.path(), "one\n", 10);
    assert!(app.editor.reload_changed_documents());
    assert_eq!(text(&app), "one\n");
    let (_, doc) = helix_view::current_ref!(app.editor);
    assert!(!doc.is_modified());
    assert!(app.editor.get_status().unwrap().0.contains("reloaded"));
    // And only once.
    assert!(!app.editor.reload_changed_documents());

    // Unsaved changes are kept; the user is warned once per change.
    send_keys(&mut app, "ix<esc>").await;
    change_on_disk(file.path(), "three\n", 20);
    assert!(app.editor.reload_changed_documents());
    assert_eq!(text(&app), "xone\n");
    let (message, severity) = app.editor.get_status().unwrap();
    assert!(message.contains("changed on disk"), "{message}");
    assert_eq!(*severity, Severity::Warning);
    assert!(!app.editor.reload_changed_documents());

    // `:reload` takes the disk's version; then it follows the file again.
    send_keys(&mut app, ":reload<ret>").await;
    assert_eq!(text(&app), "three\n");
    change_on_disk(file.path(), "four\n", 30);
    assert!(app.editor.reload_changed_documents());
    assert_eq!(text(&app), "four\n");
    app.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn own_writes_and_disabled_config_dont_reload() -> anyhow::Result<()> {
    let file = helpers::temp_file_with_contents("one\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;

    // Saving doesn't count as a change on disk.
    send_keys(&mut app, "ix<esc>:w<ret>").await;
    assert!(!app.editor.reload_changed_documents());
    assert_eq!(text(&app), "xone\n");

    let mut config = app.editor.config().clone();
    config.auto_reload = false;
    app.editor
        .config_events
        .0
        .send(helix_view::editor::ConfigEvent::Update(Box::new(config)))?;
    send_keys(&mut app, "").await;
    change_on_disk(file.path(), "two\n", 10);
    assert!(!app.editor.reload_changed_documents());
    assert_eq!(text(&app), "xone\n");
    app.close().await;
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn special_files_are_not_read() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("pipe");
    std::fs::write(&path, "one\n")?;
    let mut app = helpers::AppBuilder::new()
        .with_file(&path, None)
        .build()?;
    // Replaced by a FIFO nobody writes to: opening it would block.
    std::fs::remove_file(&path)?;
    assert!(std::process::Command::new("mkfifo")
        .arg(&path)
        .status()?
        .success());
    assert!(!app.editor.reload_changed_documents());
    assert_eq!(text(&app), "one\n");
    app.close().await;
    Ok(())
}
